use localization::Language;
use std::collections::HashMap;

use crate::url::referral_configuration;
use application::error::{BoxError, box_error, static_error};
use domain_primitives::event_id::EventId;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId, outbound_url};

use platform_postgres::SqlxTransaction;
use product_listing_core::{
    content_policy::{ContentPolicyDecision, SensitiveContentCategory},
    listing_lifecycle::ListingLifecycle,
    product_listing_event::ProductListingEventPayload,
    product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_service::ports::{
    ListingSourceSummary, ProductListingWatchlistNotificationChange,
    ProductListingWatchlistNotificationSource, ProductListingWatchlistNotificationSourceReadError,
    ProductListingWatchlistNotificationSourceReadOutcome,
    ProductListingWatchlistNotificationSourceReader,
    ProductListingWatchlistNotificationSourceReaderFactory,
};

use sqlx::PgConnection;

use super::product_listing_details_reader::images;
use crate::product_listing_event_codec;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingWatchlistNotificationSourceReaderFactory;

struct SqlxProductListingWatchlistNotificationSourceReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct SourceRow {
    event_id: uuid::Uuid,
    event_time: time::OffsetDateTime,
    product_listing_id: uuid::Uuid,
    lifecycle: String,
    event_type: String,
    event_group: String,
    event_type_schema_version: i16,
    payload: serde_json::Value,
    product_listing_title_slug_id: String,
    listing_source_id: uuid::Uuid,
    source_listing_id: String,
    listing_source_slug_id: String,
    listing_source_name: String,
    listing_source_referral_configuration: Option<serde_json::Value>,
    title_text: Option<String>,
    title_language: Option<String>,
    product_images: serde_json::Value,
    content_policy_decision: Option<String>,
    content_policy_category: Option<String>,
    url: String,
}

#[derive(Debug, sqlx::FromRow)]
struct TitleRow {
    language: String,
    title: String,
}

#[derive(Debug, thiserror::Error)]
#[error("watchlist notification source SQL query failed")]
struct WatchlistNotificationSourceQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("watchlist notification source persisted state could not be mapped")]
struct WatchlistNotificationSourceMappingError {
    #[source]
    source: BoxError,
}

impl WatchlistNotificationSourceMappingError {
    fn invalid(message: &'static str) -> Self {
        Self {
            source: static_error(message),
        }
    }

    fn with_source(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            source: box_error(source),
        }
    }
}

impl From<WatchlistNotificationSourceQueryError>
    for ProductListingWatchlistNotificationSourceReadError
{
    fn from(source: WatchlistNotificationSourceQueryError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl From<WatchlistNotificationSourceMappingError>
    for ProductListingWatchlistNotificationSourceReadError
{
    fn from(source: WatchlistNotificationSourceMappingError) -> Self {
        Self::InvalidPersistedState {
            source: box_error(source),
        }
    }
}

impl SqlxProductListingWatchlistNotificationSourceReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingWatchlistNotificationSourceReaderFactory<SqlxTransaction>
    for SqlxProductListingWatchlistNotificationSourceReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingWatchlistNotificationSourceReader + 'tx {
        SqlxProductListingWatchlistNotificationSourceReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingWatchlistNotificationSourceReader
    for SqlxProductListingWatchlistNotificationSourceReader<'_>
{
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<
        ProductListingWatchlistNotificationSourceReadOutcome,
        ProductListingWatchlistNotificationSourceReadError,
    > {
        let row = sqlx::query_as::<_, SourceRow>(
            r#"
            SELECT
                event.event_id, event.event_time, event.product_listing_id, event.event_type,
                event.event_group, event.event_type_schema_version, event.payload,
                product.lifecycle, product.product_listing_title_slug_id, product.listing_source_id, product.source_listing_id,
                listing_source.listing_source_slug_id, listing_source.name AS listing_source_name,
                listing_source.referral_configuration AS listing_source_referral_configuration,
                product.title_text, product.title_language, product.product_images,
                assessment.decision AS content_policy_decision,
                assessment.category AS content_policy_category,
                product.url
            FROM product_listing_events event
            JOIN product_listings product ON product.product_listing_id = event.product_listing_id
            JOIN listing_sources listing_source
              ON listing_source.listing_source_id = product.listing_source_id
            LEFT JOIN product_listing_content_assessments assessment
                ON assessment.product_listing_id = product.product_listing_id
                AND assessment.source_event_id = product.content_source_event_id
            WHERE event.event_id = $1 AND event.product_listing_id = $2
            FOR SHARE OF product
            "#,
        )
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(WatchlistNotificationSourceQueryError)?;
        let Some(row) = row else {
            return Ok(ProductListingWatchlistNotificationSourceReadOutcome::MissingSource);
        };
        let event = product_listing_event_codec::decode_persisted(
            &row.event_type,
            &row.event_group,
            row.event_type_schema_version,
            &row.payload,
        )
        .map_err(WatchlistNotificationSourceMappingError::with_source)?;
        let changes = notification_changes(event);
        if changes.is_empty() {
            return Ok(ProductListingWatchlistNotificationSourceReadOutcome::IgnoredEvent);
        }
        let translations = sqlx::query_as::<_, TitleRow>(
            "SELECT language, title FROM product_listing_translations WHERE product_listing_id = $1 AND title IS NOT NULL",
        )
        .bind(row.product_listing_id)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(WatchlistNotificationSourceQueryError)?;
        let mut title = HashMap::new();
        if let (Some(language), Some(text)) = (row.title_language.as_deref(), row.title_text) {
            title.insert(parse_language(language)?, Title::from(text));
        }
        for translation in translations {
            title.insert(
                parse_language(&translation.language)?,
                Title::from(translation.title),
            );
        }
        let image = images(row.product_images)
            .map_err(|_| {
                WatchlistNotificationSourceMappingError::invalid(
                    "persisted watchlist notification source images are invalid",
                )
            })?
            .into_iter()
            .next();
        let url = url::Url::parse(&row.url)
            .map_err(WatchlistNotificationSourceMappingError::with_source)?;
        let view_url = outbound_url(
            referral_configuration(row.listing_source_referral_configuration.as_ref())
                .map_err(|_| {
                    WatchlistNotificationSourceMappingError::invalid(
                        "persisted watchlist notification source referral configuration is invalid",
                    )
                })?
                .as_ref(),
            &url,
        )
        .map_err(WatchlistNotificationSourceMappingError::with_source)?;
        Ok(ProductListingWatchlistNotificationSourceReadOutcome::Found(
            ProductListingWatchlistNotificationSource {
                event_id: EventId::from(row.event_id),
                event_time: row.event_time,
                product_listing_id: ProductListingId::from(row.product_listing_id),
                lifecycle: lifecycle(&row.lifecycle)?,
                product_listing_title_slug_id: ProductListingSlugId::raw(
                    &row.product_listing_title_slug_id,
                )
                .map_err(WatchlistNotificationSourceMappingError::with_source)?,
                source: ListingSourceSummary {
                    listing_source_id: ListingSourceId::from(row.listing_source_id),
                    name: ListingSourceName::try_from(row.listing_source_name)
                        .map_err(WatchlistNotificationSourceMappingError::with_source)?,
                    slug_id: ListingSourceSlugId::raw(&row.listing_source_slug_id)
                        .map_err(WatchlistNotificationSourceMappingError::with_source)?,
                },
                source_listing_id: SourceListingId::try_from(row.source_listing_id)
                    .map_err(WatchlistNotificationSourceMappingError::with_source)?,
                title: (!title.is_empty()).then_some(title),
                image,
                content_policy: content_policy(
                    row.content_policy_decision.as_deref(),
                    row.content_policy_category.as_deref(),
                )?,
                view_url,
                url,
                changes,
            },
        ))
    }
}

fn lifecycle(
    value: &str,
) -> Result<ListingLifecycle, ProductListingWatchlistNotificationSourceReadError> {
    ListingLifecycle::from_code(value).ok_or_else(|| {
        WatchlistNotificationSourceMappingError::invalid(
            "persisted watchlist notification source lifecycle is invalid",
        )
        .into()
    })
}

fn content_policy(
    decision: Option<&str>,
    category: Option<&str>,
) -> Result<Option<ContentPolicyDecision>, ProductListingWatchlistNotificationSourceReadError> {
    match (decision, category) {
        (None, None) => Ok(None),
        (Some("ALLOWED"), None) => Ok(Some(ContentPolicyDecision::Allowed)),
        (Some("REQUIRES_CONSENT"), Some("NAZI_GERMANY")) => Ok(Some(
            ContentPolicyDecision::RequiresConsent(SensitiveContentCategory::NaziGermany),
        )),
        _ => Err(WatchlistNotificationSourceMappingError::invalid(
            "persisted watchlist notification content assessment is invalid",
        )
        .into()),
    }
}

fn notification_changes(
    event: product_listing_event_codec::ProductListingPersistedEvent,
) -> Vec<ProductListingWatchlistNotificationChange> {
    let product_listing_event_codec::ProductListingPersistedEvent::Domain(_, payload) = event
    else {
        return Vec::new();
    };
    let ProductListingEventPayload::Changed(changed) = *payload else {
        return Vec::new();
    };

    let mut changes = Vec::new();
    if let Some(price) = changed.price() {
        changes.push(ProductListingWatchlistNotificationChange::PriceChanged {
            old_price: *price.previous(),
            new_price: *price.current(),
        });
    }
    if let Some(availability) = changed.availability() {
        changes.push(
            ProductListingWatchlistNotificationChange::AvailabilityChanged {
                old_availability: *availability.previous(),
                new_availability: *availability.current(),
            },
        );
    }
    changes
}

fn parse_language(
    value: &str,
) -> Result<Language, ProductListingWatchlistNotificationSourceReadError> {
    Language::from_code(value).ok_or_else(|| {
        WatchlistNotificationSourceMappingError::invalid(
            "persisted watchlist notification source language is invalid",
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use money::Currency;
    use product_listing_core::{
        listing_availability::ListingAvailability, product_listing_price::ProductListingPrice,
    };

    #[test]
    fn should_read_price_and_availability_from_composite_changed_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let event = product_listing_event_codec::decode_persisted(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            1,
            &serde_json::json!({
                "pricing": {
                    "price": {
                        "previous": {
                            "type": "MONETARY",
                            "amount": 1200,
                            "currency": "USD"
                        },
                        "current": {
                            "type": "MONETARY",
                            "amount": 900,
                            "currency": "USD"
                        }
                    }
                },
                "availability": { "previous": "IN_STOCK", "current": "SOLD_OUT" }
            }),
        )?;
        let changes = notification_changes(event);

        assert!(matches!(
            changes.as_slice(),
            [
                ProductListingWatchlistNotificationChange::PriceChanged {
                    old_price: Some(old_price),
                    new_price: Some(new_price),
                },
                ProductListingWatchlistNotificationChange::AvailabilityChanged {
                    old_availability: Some(ListingAvailability::InStock),
                    new_availability: Some(ListingAvailability::SoldOut),
                }
            ] if matches!(
                old_price,
                ProductListingPrice::Monetary(price)
                    if u64::from(price.monetary_amount) == 1200
                        && price.currency == Currency::Usd
            ) && matches!(
                new_price,
                ProductListingPrice::Monetary(price)
                    if u64::from(price.monetary_amount) == 900
                        && price.currency == Currency::Usd
            )
        ));
        Ok(())
    }

    #[test]
    fn should_ignore_estimate_only_composite_changed_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let event = product_listing_event_codec::decode_persisted(
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            1,
            &serde_json::json!({
                "pricing": {
                    "priceEstimateMin": {
                        "previous": { "amount": 1200, "currency": "USD" },
                        "current": { "amount": 900, "currency": "USD" }
                    }
                }
            }),
        )?;
        let changes = notification_changes(event);

        assert!(changes.is_empty());
        Ok(())
    }

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: ProductListingWatchlistNotificationSourceReadError =
            WatchlistNotificationSourceQueryError(sqlx::Error::RowNotFound).into();

        let ProductListingWatchlistNotificationSourceReadError::QueryFailed { source } = error
        else {
            panic!("expected source query failure");
        };
        let query_error = source
            .downcast_ref::<WatchlistNotificationSourceQueryError>()
            .unwrap_or_else(|| panic!("expected watchlist notification query error"));
        assert!(std::error::Error::source(query_error).is_some());
    }

    #[test]
    fn should_preserve_persisted_state_mapping_source() {
        let error: ProductListingWatchlistNotificationSourceReadError =
            WatchlistNotificationSourceMappingError::invalid("invalid persisted state").into();

        let ProductListingWatchlistNotificationSourceReadError::InvalidPersistedState { source } =
            error
        else {
            panic!("expected invalid persisted state");
        };
        let mapping_error = source
            .downcast_ref::<WatchlistNotificationSourceMappingError>()
            .unwrap_or_else(|| panic!("expected watchlist notification mapping error"));
        assert!(std::error::Error::source(mapping_error).is_some());
    }
}

use crate::product_listing_event_codec;
use crate::url::referral_configuration;
use application::error::{BoxError, box_error, static_error};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;

use indexmap::IndexSet;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId, outbound_url};
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use platform_postgres::SqlxTransaction;
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle,
    product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
    product_listing_id::ProductListingId,
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_service::ports::{
    ListingSourceSummary, ProductListingSearchFilterMatchSource,
    ProductListingSearchFilterMatchSourceEventKind, ProductListingSearchFilterMatchSourceReadError,
    ProductListingSearchFilterMatchSourceReader,
    ProductListingSearchFilterMatchSourceReaderFactory, ProductListingSearchFilterMatchSourceRef,
};
use sqlx::PgConnection;
use std::collections::HashMap;

use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingSearchFilterMatchSourceReaderFactory;

struct SqlxProductListingSearchFilterMatchSourceReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SourceRow {
    event_id: uuid::Uuid,
    event_type: String,
    event_group: String,
    event_type_schema_version: i16,
    payload: serde_json::Value,
    origin_event_time: OffsetDateTime,
    current_event_id: uuid::Uuid,
    projection_version: i64,
    product_listing_id: uuid::Uuid,
    product_listing_title_slug_id: String,
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    listing_source_name: String,
    listing_source_referral_configuration: Option<serde_json::Value>,
    source_listing_id: String,
    product_title_text: Option<String>,
    product_title_language: Option<String>,
    product_description_text: Option<String>,
    product_description_language: Option<String>,
    price_kind: Option<String>,
    price_amount: Option<i64>,
    price_currency: Option<String>,
    price_estimate_min_amount: Option<i64>,
    price_estimate_min_currency: Option<String>,
    price_estimate_max_amount: Option<i64>,
    price_estimate_max_currency: Option<String>,
    sale_observation_fx_rate_id: Option<uuid::Uuid>,
    sale_observed_at: Option<OffsetDateTime>,
    availability: Option<String>,
    lifecycle: String,
    url: String,
    product_images: serde_json::Value,
    embedding: Option<Vec<f32>>,
    auction_start: Option<OffsetDateTime>,
    auction_end: Option<OffsetDateTime>,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    translation_language: Option<String>,
    translation_title: Option<String>,
    translation_description: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product search-filter match source SQL query failed")]
struct SourceQuerySqlxError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product search-filter match source row is invalid")]
struct SourceRowMappingError {
    #[source]
    source: BoxError,
}

impl SourceRowMappingError {
    fn invalid(message: &'static str) -> Self {
        Self {
            source: static_error(message),
        }
    }
}

impl From<SourceQuerySqlxError> for ProductListingSearchFilterMatchSourceReadError {
    fn from(source: SourceQuerySqlxError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl From<SourceRowMappingError> for ProductListingSearchFilterMatchSourceReadError {
    fn from(source: SourceRowMappingError) -> Self {
        Self::InvalidPersistedState {
            source: box_error(source),
        }
    }
}

impl SqlxProductListingSearchFilterMatchSourceReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingSearchFilterMatchSourceReaderFactory<SqlxTransaction>
    for SqlxProductListingSearchFilterMatchSourceReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingSearchFilterMatchSourceReader + 'tx {
        SqlxProductListingSearchFilterMatchSourceReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingSearchFilterMatchSourceReader
    for SqlxProductListingSearchFilterMatchSourceReader<'_>
{
    async fn find_source(
        &mut self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<
        Option<ProductListingSearchFilterMatchSource>,
        ProductListingSearchFilterMatchSourceReadError,
    > {
        let reference = ProductListingSearchFilterMatchSourceRef {
            product_listing_id,
            event_id,
        };
        Ok(self.find_sources(&[reference]).await?.remove(&reference))
    }

    async fn find_sources(
        &mut self,
        refs: &[ProductListingSearchFilterMatchSourceRef],
    ) -> Result<
        HashMap<ProductListingSearchFilterMatchSourceRef, ProductListingSearchFilterMatchSource>,
        ProductListingSearchFilterMatchSourceReadError,
    > {
        if refs.is_empty() {
            return Ok(HashMap::new());
        }

        let product_listing_ids = refs
            .iter()
            .map(|reference| uuid::Uuid::from(reference.product_listing_id))
            .collect::<Vec<_>>();
        let event_ids = refs
            .iter()
            .map(|reference| uuid::Uuid::from(reference.event_id))
            .collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, SourceRow>(
            r#"
            WITH requested_events AS (
                SELECT DISTINCT product_listing_id, event_id
                FROM UNNEST($1::uuid[], $2::uuid[]) AS requested(product_listing_id, event_id)
            )
            SELECT
                event.event_id,
                event.event_type,
                event.event_group,
                event.event_type_schema_version,
                event.payload,
                event.event_time AS origin_event_time,
                product.current_event_id,
                product.projection_version,
                product.product_listing_id,
                product.product_listing_title_slug_id,
                listing_source.listing_source_id,
                listing_source.listing_source_slug_id,
                listing_source.name AS listing_source_name,
                listing_source.referral_configuration AS listing_source_referral_configuration,
                product.source_listing_id,
                product.title_text AS product_title_text,
                product.title_language AS product_title_language,
                product.description_text AS product_description_text,
                product.description_language AS product_description_language,
                product.price_kind,
                product.price_amount,
                product.price_currency,
                product.price_estimate_min_amount,
                product.price_estimate_min_currency,
                product.price_estimate_max_amount,
                product.price_estimate_max_currency,
                product.sale_observation_fx_rate_id,
                product.sale_observed_at,
                product.availability,
                product.lifecycle,
                product.url,
                product.product_images,
                product.embedding,
                product.auction_start,
                product.auction_end,
                product.created,
                product.updated,
                translation.language AS translation_language,
                translation.title AS translation_title,
                translation.description AS translation_description
            FROM requested_events requested
            JOIN product_listing_events event
              ON event.product_listing_id = requested.product_listing_id
             AND event.event_id = requested.event_id
            JOIN product_listings product ON product.product_listing_id = event.product_listing_id
            JOIN listing_sources listing_source
              ON listing_source.listing_source_id = product.listing_source_id
            LEFT JOIN product_listing_translations translation ON translation.product_listing_id = product.product_listing_id
            ORDER BY event.product_listing_id ASC, event.event_id ASC, translation.language ASC
            FOR SHARE OF product
            "#,
        )
        .bind(product_listing_ids)
        .bind(event_ids)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(SourceQuerySqlxError)?;

        sources_from_rows(rows).map_err(Into::into)
    }
}

fn sources_from_rows(
    rows: Vec<SourceRow>,
) -> Result<
    HashMap<ProductListingSearchFilterMatchSourceRef, ProductListingSearchFilterMatchSource>,
    SourceRowMappingError,
> {
    let mut grouped_rows =
        HashMap::<ProductListingSearchFilterMatchSourceRef, Vec<SourceRow>>::new();
    for row in rows {
        let reference = ProductListingSearchFilterMatchSourceRef {
            product_listing_id: ProductListingId::from(row.product_listing_id),
            event_id: EventId::from(row.event_id),
        };
        grouped_rows.entry(reference).or_default().push(row);
    }

    grouped_rows
        .into_iter()
        .map(|(reference, rows)| {
            let source = source_from_rows(rows)?.ok_or_else(|| {
                SourceRowMappingError::invalid(
                    "persisted product search-filter match source row is missing",
                )
            })?;
            Ok((reference, source))
        })
        .collect()
}

fn source_from_rows(
    rows: Vec<SourceRow>,
) -> Result<Option<ProductListingSearchFilterMatchSource>, SourceRowMappingError> {
    let Some(row) = rows.first() else {
        return Ok(None);
    };

    let product_title = localized_title(
        row.product_title_text.as_deref(),
        row.product_title_language.as_deref(),
    )?;
    let product_description = localized_description(
        row.product_description_text.as_deref(),
        row.product_description_language.as_deref(),
    )?;
    let source_listing_id =
        SourceListingId::try_from(row.source_listing_id.clone()).map_err(|_| {
            SourceRowMappingError::invalid(
                "persisted product search-filter match source listing ID is invalid",
            )
        })?;
    let (titles, descriptions) =
        translations(&rows, product_title.as_ref(), product_description.as_ref())?;
    let pricing = ProductListingPricing {
        price: product_listing_price(
            row.price_kind.as_deref(),
            row.price_amount,
            row.price_currency.as_deref(),
        )?,
        price_estimate_min: price(
            row.price_estimate_min_amount,
            row.price_estimate_min_currency.as_deref(),
        )?,
        price_estimate_max: price(
            row.price_estimate_max_amount,
            row.price_estimate_max_currency.as_deref(),
        )?,
    };
    let sale_observation = sale_observation(row.sale_observation_fx_rate_id, row.sale_observed_at)?;
    let images = images(&row.product_images)?;
    let url = Url::parse(&row.url).map_err(|_| {
        SourceRowMappingError::invalid(
            "persisted product search-filter match source URL is invalid",
        )
    })?;
    let referral_configuration = referral_configuration(
        row.listing_source_referral_configuration.as_ref(),
    )
    .map_err(|_| {
        SourceRowMappingError::invalid(
            "persisted product search-filter match referral configuration is invalid",
        )
    })?;
    let view_url = outbound_url(referral_configuration.as_ref(), &url).map_err(|_| {
        SourceRowMappingError::invalid(
            "persisted product search-filter match source view URL is invalid",
        )
    })?;

    let event_kind = event_kind_from_row(row)?;
    Ok(Some(ProductListingSearchFilterMatchSource {
        event_id: EventId::from(row.event_id),
        event_kind,
        origin_event_time: row.origin_event_time,
        current_event_id: EventId::from(row.current_event_id),
        projection_version: row.projection_version,
        product_listing_id: ProductListingId::from(row.product_listing_id),
        product_listing_title_slug_id: ProductListingSlugId::raw(
            &row.product_listing_title_slug_id,
        )
        .map_err(|_| {
            SourceRowMappingError::invalid(
                "persisted product search-filter match source title slug is invalid",
            )
        })?,
        source: ListingSourceSummary {
            listing_source_id: ListingSourceId::from(row.listing_source_id),
            name: ListingSourceName::try_from(row.listing_source_name.clone()).map_err(|_| {
                SourceRowMappingError::invalid(
                    "persisted product search-filter match source listing source name is invalid",
                )
            })?,
            slug_id: ListingSourceSlugId::raw(&row.listing_source_slug_id).map_err(|_| {
                SourceRowMappingError::invalid(
                    "persisted product search-filter match source listing source slug is invalid",
                )
            })?,
        },
        source_listing_id,
        product_title,
        product_description,
        titles,
        descriptions,
        pricing,
        sale_observation,
        availability: availability(row.availability.as_deref())?,
        lifecycle: lifecycle(&row.lifecycle)?,
        view_url,
        url,
        image: images.iter().next().cloned(),
        images,
        embedding: row.embedding.clone(),
        auction: ProductListingAuction {
            start: row.auction_start,
            end: row.auction_end,
        },
        created: row.created,
        updated: row.updated,
    }))
}

type LocalizedTexts = (HashMap<Language, Title>, HashMap<Language, Description>);

fn translations(
    rows: &[SourceRow],
    product_title: Option<&Localized<Language, Title>>,
    product_description: Option<&Localized<Language, Description>>,
) -> Result<LocalizedTexts, SourceRowMappingError> {
    let mut titles = HashMap::new();
    let mut descriptions = HashMap::new();

    for row in rows {
        match (
            row.translation_language.as_deref(),
            row.translation_title.as_deref(),
            row.translation_description.as_deref(),
        ) {
            (None, None, None) => {}
            (Some(language_value), Some(title_value), description_value) => {
                let language = language(language_value)?;
                titles.insert(language, title(title_value)?);
                if let Some(text) = description_value {
                    descriptions.insert(language, description(text)?);
                }
            }
            (Some(language_value), None, Some(description_value)) => {
                descriptions.insert(language(language_value)?, description(description_value)?);
            }
            _ => {
                return Err(SourceRowMappingError::invalid(
                    "persisted product search-filter match translation is incomplete",
                ));
            }
        }
    }

    if let Some(title) = product_title {
        titles
            .entry(title.localization)
            .or_insert_with(|| title.payload.clone());
    }
    if let Some(description) = product_description {
        descriptions
            .entry(description.localization)
            .or_insert_with(|| description.payload.clone());
    }

    Ok((titles, descriptions))
}

fn localized_title(
    text: Option<&str>,
    language_value: Option<&str>,
) -> Result<Option<Localized<Language, Title>>, SourceRowMappingError> {
    match (text, language_value) {
        (Some(text), Some(language_value)) => Ok(Some(Localized::new(
            language(language_value)?,
            title(text)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(SourceRowMappingError::invalid(
            "persisted product search-filter match title is incomplete",
        )),
    }
}

fn localized_description(
    text: Option<&str>,
    language_value: Option<&str>,
) -> Result<Option<Localized<Language, Description>>, SourceRowMappingError> {
    match (text, language_value) {
        (Some(text), Some(language_value)) => Ok(Some(Localized::new(
            language(language_value)?,
            description(text)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(SourceRowMappingError::invalid(
            "persisted product search-filter match description is incomplete",
        )),
    }
}

fn title(value: &str) -> Result<Title, SourceRowMappingError> {
    let title = Title::from(value);
    (!title.as_ref().is_empty() && title.as_ref() == value)
        .then_some(title)
        .ok_or_else(|| {
            SourceRowMappingError::invalid("persisted product search-filter match title is invalid")
        })
}

fn description(value: &str) -> Result<Description, SourceRowMappingError> {
    let description = Description::from(value);
    (!description.as_ref().is_empty() && description.as_ref() == value)
        .then_some(description)
        .ok_or_else(|| {
            SourceRowMappingError::invalid(
                "persisted product search-filter match description is invalid",
            )
        })
}

fn product_listing_price(
    kind: Option<&str>,
    amount: Option<i64>,
    currency_value: Option<&str>,
) -> Result<
    Option<product_listing_core::product_listing_price::ProductListingPrice>,
    SourceRowMappingError,
> {
    match (kind, amount, currency_value) {
        (None, None, None) => Ok(None),
        (Some("MONETARY"), Some(amount), Some(currency_value)) => Ok(Some(
            product_listing_core::product_listing_price::ProductListingPrice::Monetary(Price::new(
                MonetaryAmount::from(u64::try_from(amount).map_err(|_| {
                    SourceRowMappingError::invalid(
                        "persisted product search-filter match price amount is invalid",
                    )
                })?),
                currency(currency_value)?,
            )),
        )),
        (Some("ON_REQUEST"), None, None) => Ok(Some(
            product_listing_core::product_listing_price::ProductListingPrice::OnRequest,
        )),
        _ => Err(SourceRowMappingError::invalid(
            "persisted product search-filter match main price is invalid",
        )),
    }
}

fn price(
    amount: Option<i64>,
    currency_value: Option<&str>,
) -> Result<Option<Price>, SourceRowMappingError> {
    match (amount, currency_value) {
        (Some(amount), Some(currency_value)) => Ok(Some(Price::new(
            MonetaryAmount::from(u64::try_from(amount).map_err(|_| {
                SourceRowMappingError::invalid(
                    "persisted product search-filter match price amount is invalid",
                )
            })?),
            currency(currency_value)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(SourceRowMappingError::invalid(
            "persisted product search-filter match price is incomplete",
        )),
    }
}

fn sale_observation(
    fx_rate_id: Option<uuid::Uuid>,
    observed_at: Option<OffsetDateTime>,
) -> Result<Option<ListingSaleObservation>, SourceRowMappingError> {
    match (fx_rate_id, observed_at) {
        (Some(fx_rate_id), Some(observed_at)) => Ok(Some(ListingSaleObservation::new(
            observed_at,
            FxRateId::from(fx_rate_id),
        ))),
        (None, None) => Ok(None),
        _ => Err(SourceRowMappingError::invalid(
            "persisted product search-filter match sale observation is incomplete",
        )),
    }
}

fn images(
    value: &serde_json::Value,
) -> Result<IndexSet<ProductListingImage>, SourceRowMappingError> {
    #[derive(serde::Deserialize)]
    struct ImageJson {
        url: String,
    }

    serde_json::from_value::<Vec<ImageJson>>(value.clone())
        .map_err(|_| {
            SourceRowMappingError::invalid(
                "persisted product search-filter match images are invalid",
            )
        })?
        .into_iter()
        .map(|image| {
            Ok(ProductListingImage::new(Url::parse(&image.url).map_err(
                |_| {
                    SourceRowMappingError::invalid(
                        "persisted product search-filter match image URL is invalid",
                    )
                },
            )?))
        })
        .collect()
}

fn language(value: &str) -> Result<Language, SourceRowMappingError> {
    Language::from_code(value).ok_or_else(|| {
        SourceRowMappingError::invalid("persisted product search-filter match language is invalid")
    })
}

fn currency(value: &str) -> Result<Currency, SourceRowMappingError> {
    Currency::from_code(value).ok_or_else(|| {
        SourceRowMappingError::invalid("persisted product search-filter match currency is invalid")
    })
}

fn availability(value: Option<&str>) -> Result<Option<ListingAvailability>, SourceRowMappingError> {
    value
        .map(|value| {
            ListingAvailability::from_code(value).ok_or_else(|| {
                SourceRowMappingError::invalid(
                    "persisted product search-filter match availability is invalid",
                )
            })
        })
        .transpose()
}

fn lifecycle(value: &str) -> Result<ListingLifecycle, SourceRowMappingError> {
    ListingLifecycle::from_code(value).ok_or_else(|| {
        SourceRowMappingError::invalid("persisted product search-filter match lifecycle is invalid")
    })
}

fn event_kind_from_row(
    row: &SourceRow,
) -> Result<ProductListingSearchFilterMatchSourceEventKind, SourceRowMappingError> {
    let event = product_listing_event_codec::decode_persisted(
        &row.event_type,
        &row.event_group,
        row.event_type_schema_version,
        &row.payload,
    )
    .map_err(|source| SourceRowMappingError {
        source: box_error(source),
    })?;
    Ok(match event {
        product_listing_event_codec::ProductListingPersistedEvent::Domain(_, _) => {
            ProductListingSearchFilterMatchSourceEventKind::Domain
        }
        product_listing_event_codec::ProductListingPersistedEvent::Embedded
        | product_listing_event_codec::ProductListingPersistedEvent::TranslatedTitles => {
            ProductListingSearchFilterMatchSourceEventKind::Enrichment
        }
    })
}

#[cfg(test)]
fn event_kind(value: &str) -> ProductListingSearchFilterMatchSourceEventKind {
    match value {
        "DOMAIN" => ProductListingSearchFilterMatchSourceEventKind::Domain,
        "ENRICHMENT" => ProductListingSearchFilterMatchSourceEventKind::Enrichment,
        _ => ProductListingSearchFilterMatchSourceEventKind::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: ProductListingSearchFilterMatchSourceReadError =
            SourceQuerySqlxError(sqlx::Error::RowNotFound).into();

        let ProductListingSearchFilterMatchSourceReadError::QueryFailed { source } = error else {
            panic!("expected source query failure");
        };
        assert!(source.downcast_ref::<SourceQuerySqlxError>().is_some());
        assert!(source.source().is_some());
    }

    #[test]
    fn should_preserve_row_mapping_source() {
        let error: ProductListingSearchFilterMatchSourceReadError =
            SourceRowMappingError::invalid("invalid persisted state").into();

        let ProductListingSearchFilterMatchSourceReadError::InvalidPersistedState { source } =
            error
        else {
            panic!("expected invalid persisted state");
        };
        let mapping_error = source
            .downcast_ref::<SourceRowMappingError>()
            .unwrap_or_else(|| panic!("expected source row mapping error"));
        assert!(std::error::Error::source(mapping_error).is_some());
    }

    #[test]
    fn should_classify_percolation_event_groups() {
        assert_eq!(
            ProductListingSearchFilterMatchSourceEventKind::Domain,
            event_kind("DOMAIN")
        );
        assert_eq!(
            ProductListingSearchFilterMatchSourceEventKind::Enrichment,
            event_kind("ENRICHMENT")
        );
        assert_eq!(
            ProductListingSearchFilterMatchSourceEventKind::Ignored,
            event_kind("UNKNOWN")
        );
    }

    #[test]
    fn should_map_sale_observation_only_when_both_persisted_columns_are_present() {
        assert!(matches!(sale_observation(None, None), Ok(None)));

        let fx_rate_id = uuid::Uuid::new_v4();
        let observed_at = OffsetDateTime::UNIX_EPOCH;
        assert!(matches!(
            sale_observation(Some(fx_rate_id), Some(observed_at)),
            Ok(Some(value))
                if value == ListingSaleObservation::new(observed_at, FxRateId::from(fx_rate_id))
        ));

        assert!(sale_observation(Some(uuid::Uuid::new_v4()), None).is_err());
        assert!(sale_observation(None, Some(observed_at)).is_err());
    }

    #[test]
    fn should_reject_noncanonical_persisted_values() {
        assert!(language("EN").is_err());
        assert!(currency("eur").is_err());
        assert!(availability(Some("available")).is_err());
        assert!(lifecycle("active").is_err());
    }

    #[test]
    fn should_reject_noncanonical_localized_text() {
        assert!(title(" title ").is_err());
        assert!(description(" ").is_err());
    }
}

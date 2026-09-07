#![allow(dead_code)]

use application::error::box_error;
use domain_primitives::event_id::EventId;
use domain_primitives::versioned::Versioned;
use fxrate_core::FxRateId;

use indexmap::IndexSet;
use localization::Language;
use localization::Localized;
use money::Currency;
use money::{MonetaryAmount, Price};
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{
    ListingSaleObservation, ProductListing, ProductListingAuction, ProductListingPricing,
    RehydratedProductListingState,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use listing_source_core::ListingSourceId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use product_listing_service::ports::product_listing_repository::{
    ProductListingRepository, ProductListingRepositoryError, ProductListingRepositoryFactory,
    ProductListingStorageVersion, ProductListingWriteEffects, VersionedProductListing,
};
use serde::{Deserialize, Serialize};

use sqlx::PgConnection;

use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingRepositoryFactory;

struct SqlxProductListingRepository<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingRow {
    product_listing_id: uuid::Uuid,
    product_listing_title_slug_id: String,
    version: i64,
    current_event_id: uuid::Uuid,
    listing_source_id: uuid::Uuid,
    source_listing_id: String,
    title_text: Option<String>,
    title_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
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
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductListingImageJson {
    url: String,
}

impl SqlxProductListingRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingRepositoryFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingRepository + 'tx {
        SqlxProductListingRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingRepository for SqlxProductListingRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        let row = sqlx::query_as::<_, ProductListingRow>(
            r#"
            SELECT
                product_listing_id, product_listing_title_slug_id, version, current_event_id, listing_source_id, source_listing_id,
                title_text, title_language, description_text, description_language,
                price_amount, price_currency, price_estimate_min_amount,
                price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_observation_fx_rate_id, sale_observed_at, availability, lifecycle, url,
                product_images, embedding, auction_start, auction_end, created, updated
            FROM product_listings
            WHERE product_listing_id = $1
            "#,
        )
        .bind(uuid::Uuid::from(id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingLookupByIdSqlxError)?;

        row.map(TryInto::try_into).transpose()
    }

    async fn find_by_key(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        let row = sqlx::query_as::<_, ProductListingRow>(
            r#"
            SELECT
                product_listing_id, product_listing_title_slug_id, version, current_event_id, listing_source_id, source_listing_id,
                title_text, title_language, description_text, description_language,
                price_amount, price_currency, price_estimate_min_amount,
                price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_observation_fx_rate_id, sale_observed_at, availability, lifecycle, url,
                product_images, embedding, auction_start, auction_end, created, updated
            FROM product_listings
            WHERE listing_source_id = $1
              AND source_listing_id = $2
            "#,
        )
        .bind(uuid::Uuid::from(key.listing_source_id))
        .bind(key.source_listing_id.as_ref())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingLookupByKeySqlxError)?;

        row.map(TryInto::try_into).transpose()
    }

    async fn insert(
        &mut self,
        product: &ProductListing,
        current_event_id: EventId,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
        let pricing = product.pricing();
        let auction = product.auction();
        let title = product.title();
        let description = product.description();
        let price_amount = pricing
            .price
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductListingRepositoryError::ProductListingInsertFailed)?;
        let price_estimate_min_amount = pricing
            .price_estimate_min
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductListingRepositoryError::ProductListingInsertFailed)?;
        let price_estimate_max_amount = pricing
            .price_estimate_max
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductListingRepositoryError::ProductListingInsertFailed)?;
        let product_images = images_to_json(product.images())
            .map_err(|_| ProductListingRepositoryError::ProductListingInsertFailed)?;
        let version = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO product_listings (
                product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id,
                embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language,
                description_text, description_language, price_amount, price_currency,
                price_estimate_min_amount, price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_observation_fx_rate_id, sale_observed_at,
                availability, lifecycle, url, product_images, auction_start, auction_end
            ) VALUES (
                $1, $2, $3, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                $14, $15, $16, $17, $18, $19, $20, $21, $22, $23
            )
            RETURNING version
            "#,
        )
        .bind(uuid::Uuid::from(product.id()))
        .bind(product.title_slug_id().as_ref().to_owned())
        .bind(uuid::Uuid::from(current_event_id))
        .bind(uuid::Uuid::from(product.listing_source_id()))
        .bind(product.source_listing_id().as_ref().to_owned())
        .bind(title.map(|value| value.payload.as_ref().to_owned()))
        .bind(title.map(|value| value.localization.as_str().to_owned()))
        .bind(description.map(|value| value.payload.as_ref().to_owned()))
        .bind(description.map(|value| value.localization.as_str().to_owned()))
        .bind(price_amount)
        .bind(
            pricing
                .price
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(price_estimate_min_amount)
        .bind(
            pricing
                .price_estimate_min
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(price_estimate_max_amount)
        .bind(
            pricing
                .price_estimate_max
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(
            product
                .sale_observation()
                .map(|value| uuid::Uuid::from(value.fx_rate_id())),
        )
        .bind(product.sale_observation().map(|value| value.observed_at()))
        .bind(product.availability().map(ListingAvailability::as_str))
        .bind(product.lifecycle().as_str())
        .bind(product.url().to_string())
        .bind(product_images)
        .bind(auction.start)
        .bind(auction.end)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(ProductListingInsertSqlxError)?;

        let version = ProductListingStorageVersion::try_from(version)
            .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?;
        Ok(Versioned::new(product.clone(), version))
    }

    async fn update(
        &mut self,
        product: &ProductListing,
        expected_version: ProductListingStorageVersion,
        current_event_id: EventId,
        effects: ProductListingWriteEffects,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
        let pricing = product.pricing();
        let auction = product.auction();
        let title = product.title();
        let description = product.description();
        let price_amount = pricing
            .price
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductListingRepositoryError::ProductListingUpdateFailed)?;
        let price_estimate_min_amount = pricing
            .price_estimate_min
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductListingRepositoryError::ProductListingUpdateFailed)?;
        let price_estimate_max_amount = pricing
            .price_estimate_max
            .map(|value| amount_to_i64(value.monetary_amount))
            .transpose()
            .map_err(|_| ProductListingRepositoryError::ProductListingUpdateFailed)?;
        let product_images = images_to_json(product.images())
            .map_err(|_| ProductListingRepositoryError::ProductListingUpdateFailed)?;
        let expected_version = i64::try_from(expected_version.into_inner())
            .map_err(|_| ProductListingRepositoryError::ProductListingUpdateFailed)?;
        let version = sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE product_listings
            SET
                current_event_id = $1,
                embedding_source_event_id = CASE WHEN $2 THEN $1 ELSE embedding_source_event_id END,
                embedding = CASE WHEN $2 THEN NULL ELSE embedding END,
                title_text = $3,
                title_language = $4,
                description_text = $5,
                description_language = $6,
                price_amount = $7,
                price_currency = $8,
                price_estimate_min_amount = $9,
                price_estimate_min_currency = $10,
                price_estimate_max_amount = $11,
                price_estimate_max_currency = $12,
                sale_observation_fx_rate_id = $13,
                sale_observed_at = $14,
                availability = $15,
                lifecycle = $16,
                url = $17,
                product_images = $18,
                auction_start = $19,
                auction_end = $20,
                version = version + 1,
                projection_version = projection_version + 1,
                updated = now()
            WHERE product_listing_id = $21 AND version = $22
            RETURNING version
            "#,
        )
        .bind(uuid::Uuid::from(current_event_id))
        .bind(effects.advance_embedding_source)
        .bind(title.map(|value| value.payload.as_ref().to_owned()))
        .bind(title.map(|value| value.localization.as_str().to_owned()))
        .bind(description.map(|value| value.payload.as_ref().to_owned()))
        .bind(description.map(|value| value.localization.as_str().to_owned()))
        .bind(price_amount)
        .bind(
            pricing
                .price
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(price_estimate_min_amount)
        .bind(
            pricing
                .price_estimate_min
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(price_estimate_max_amount)
        .bind(
            pricing
                .price_estimate_max
                .map(|value| value.currency.as_str().to_owned()),
        )
        .bind(
            product
                .sale_observation()
                .map(|value| uuid::Uuid::from(value.fx_rate_id())),
        )
        .bind(product.sale_observation().map(|value| value.observed_at()))
        .bind(product.availability().map(ListingAvailability::as_str))
        .bind(product.lifecycle().as_str())
        .bind(product.url().to_string())
        .bind(product_images)
        .bind(auction.start)
        .bind(auction.end)
        .bind(uuid::Uuid::from(product.id()))
        .bind(expected_version)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingUpdateSqlxError)?
        .ok_or(ProductListingRepositoryError::ConcurrencyConflict)?;

        let version = ProductListingStorageVersion::try_from(version)
            .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?;
        Ok(Versioned::new(product.clone(), version))
    }
}

impl TryFrom<ProductListingRow> for VersionedProductListing {
    type Error = ProductListingRepositoryError;

    fn try_from(row: ProductListingRow) -> Result<Self, Self::Error> {
        let _created = row.created;
        let _updated = row.updated;
        let title = localized_title_from_row(&row)?;
        let description = localized_description_from_row(&row)?;
        let source_listing_id = SourceListingId::try_from(row.source_listing_id)
            .map_err(|_| ProductListingRepositoryError::InvalidSourceListingIdPersisted)?;
        let product = ProductListing::rehydrate(RehydratedProductListingState {
            id: ProductListingId::from(row.product_listing_id),
            title_slug_id: ProductListingSlugId::raw(&row.product_listing_title_slug_id)
                .map_err(|_| ProductListingRepositoryError::InvalidProductListingSlugPersisted)?,
            listing_source_id: ListingSourceId::from(row.listing_source_id),
            source_listing_id,
            title,
            description,
            pricing: ProductListingPricing {
                price: price_from_parts(row.price_amount, row.price_currency)?,
                price_estimate_min: price_from_parts(
                    row.price_estimate_min_amount,
                    row.price_estimate_min_currency,
                )?,
                price_estimate_max: price_from_parts(
                    row.price_estimate_max_amount,
                    row.price_estimate_max_currency,
                )?,
            },
            sale_observation: sale_observation_from_parts(
                row.sale_observed_at,
                row.sale_observation_fx_rate_id,
            )?,
            availability: parse_listing_availability(row.availability.as_deref())?,
            lifecycle: parse_listing_lifecycle(&row.lifecycle)?,
            url: Url::parse(&row.url)
                .map_err(|_| ProductListingRepositoryError::InvalidProductListingUrlPersisted)?,
            images: images_from_json(row.product_images)?,
            auction: ProductListingAuction {
                start: row.auction_start,
                end: row.auction_end,
            },
        })
        .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?;

        let _current_event_id = EventId::from(row.current_event_id);
        Ok(Versioned {
            value: product,
            version: ProductListingStorageVersion::try_from(row.version)
                .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?,
        })
    }
}

fn sale_observation_from_parts(
    observed_at: Option<OffsetDateTime>,
    fx_rate_id: Option<uuid::Uuid>,
) -> Result<Option<ListingSaleObservation>, ProductListingRepositoryError> {
    match (observed_at, fx_rate_id) {
        (Some(observed_at), Some(fx_rate_id)) => Ok(Some(ListingSaleObservation::new(
            observed_at,
            FxRateId::from(fx_rate_id),
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductListingRepositoryError::InvalidAggregateStatePersisted),
    }
}

fn amount_to_i64(amount: MonetaryAmount) -> Result<i64, ()> {
    i64::try_from(u64::from(amount)).map_err(|_| ())
}

fn price_from_parts(
    amount: Option<i64>,
    currency: Option<String>,
) -> Result<Option<Price>, ProductListingRepositoryError> {
    match (amount, currency) {
        (Some(amount), Some(currency)) => {
            let amount = u64::try_from(amount)
                .map_err(|_| ProductListingRepositoryError::NegativePriceAmountPersisted)?;
            Ok(Some(Price::new(
                MonetaryAmount::from(amount),
                parse_currency(&currency)?,
            )))
        }
        (None, None) => Ok(None),
        _ => Err(ProductListingRepositoryError::IncompletePricePersisted),
    }
}

fn localized_title_from_row(
    row: &ProductListingRow,
) -> Result<Option<Localized<Language, Title>>, ProductListingRepositoryError> {
    match (&row.title_text, &row.title_language) {
        (Some(text), Some(language)) => Ok(Some(Localized::new(
            parse_title_language(language)?,
            Title::from(text),
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductListingRepositoryError::IncompleteTitlePersisted),
    }
}

fn localized_description_from_row(
    row: &ProductListingRow,
) -> Result<Option<Localized<Language, Description>>, ProductListingRepositoryError> {
    match (&row.description_text, &row.description_language) {
        (Some(text), Some(language)) => Ok(Some(Localized::new(
            parse_description_language(language)?,
            Description::from(text),
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductListingRepositoryError::IncompleteDescriptionPersisted),
    }
}

fn images_to_json(images: &IndexSet<ProductListingImage>) -> Result<serde_json::Value, ()> {
    let images = images
        .iter()
        .map(|image| ProductListingImageJson {
            url: image.url().to_string(),
        })
        .collect::<Vec<_>>();
    serde_json::to_value(images).map_err(|_| ())
}

fn images_from_json(
    value: serde_json::Value,
) -> Result<IndexSet<ProductListingImage>, ProductListingRepositoryError> {
    let images: Vec<ProductListingImageJson> = serde_json::from_value(value)
        .map_err(|_| ProductListingRepositoryError::InvalidProductListingImagesPersisted)?;
    images
        .into_iter()
        .map(|image| {
            Ok(ProductListingImage::new(Url::parse(&image.url).map_err(
                |_| ProductListingRepositoryError::InvalidProductListingImageUrlPersisted,
            )?))
        })
        .collect()
}

fn parse_title_language(value: &str) -> Result<Language, ProductListingRepositoryError> {
    parse_language(
        value,
        ProductListingRepositoryError::InvalidTitleLanguagePersisted,
    )
}

fn parse_description_language(value: &str) -> Result<Language, ProductListingRepositoryError> {
    parse_language(
        value,
        ProductListingRepositoryError::InvalidDescriptionLanguagePersisted,
    )
}

fn parse_language(
    value: &str,
    error: ProductListingRepositoryError,
) -> Result<Language, ProductListingRepositoryError> {
    Language::from_code(value).ok_or(error)
}

fn parse_currency(value: &str) -> Result<Currency, ProductListingRepositoryError> {
    Currency::from_code(value).ok_or(ProductListingRepositoryError::InvalidPriceCurrencyPersisted)
}

fn parse_listing_availability(
    value: Option<&str>,
) -> Result<Option<ListingAvailability>, ProductListingRepositoryError> {
    value
        .map(|value| {
            ListingAvailability::from_code(value)
                .ok_or(ProductListingRepositoryError::InvalidListingAvailabilityPersisted)
        })
        .transpose()
}

fn parse_listing_lifecycle(value: &str) -> Result<ListingLifecycle, ProductListingRepositoryError> {
    ListingLifecycle::from_code(value)
        .ok_or(ProductListingRepositoryError::InvalidListingLifecyclePersisted)
}

struct ProductListingLookupByIdSqlxError(sqlx::Error);
#[derive(Debug, thiserror::Error)]
#[error("product lookup by listing-source identity query failed")]
struct ProductListingLookupByKeySqlxError(#[source] sqlx::Error);
struct ProductListingInsertSqlxError(sqlx::Error);
struct ProductListingUpdateSqlxError(sqlx::Error);

impl From<ProductListingLookupByIdSqlxError> for ProductListingRepositoryError {
    fn from(value: ProductListingLookupByIdSqlxError) -> Self {
        let ProductListingLookupByIdSqlxError(_error) = value;
        Self::ProductListingLookupByIdFailed
    }
}

impl From<ProductListingLookupByKeySqlxError> for ProductListingRepositoryError {
    fn from(error: ProductListingLookupByKeySqlxError) -> Self {
        Self::ProductListingLookupByKeyFailed {
            source: box_error(error),
        }
    }
}

impl From<ProductListingInsertSqlxError> for ProductListingRepositoryError {
    fn from(error: ProductListingInsertSqlxError) -> Self {
        match error.0 {
            sqlx::Error::Database(db_error)
                if db_error.constraint()
                    == Some("product_listings_listing_source_listing_unique") =>
            {
                Self::SourceListingAlreadyExists
            }
            sqlx::Error::Database(db_error)
                if db_error.constraint() == Some("product_listings_title_slug_unique") =>
            {
                Self::ProductListingTitleSlugAlreadyExists
            }
            _ => Self::ProductListingInsertFailed,
        }
    }
}

impl From<ProductListingUpdateSqlxError> for ProductListingRepositoryError {
    fn from(value: ProductListingUpdateSqlxError) -> Self {
        let ProductListingUpdateSqlxError(error) = value;
        match &error {
            sqlx::Error::Database(db_error)
                if db_error.constraint()
                    == Some("product_listings_listing_source_listing_unique") =>
            {
                Self::SourceListingAlreadyExists
            }
            _ => Self::ProductListingUpdateFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use strum::IntoEnumIterator;

    #[test]
    fn should_preserve_key_lookup_sqlx_source() {
        let error =
            ProductListingLookupByKeySqlxError(sqlx::Error::Protocol("test failure".to_owned()));

        let mapped: ProductListingRepositoryError = error.into();

        let source = match mapped {
            ProductListingRepositoryError::ProductListingLookupByKeyFailed { source } => source,
            error => panic!("unexpected error: {error:?}"),
        };
        assert_eq!(
            "product lookup by listing-source identity query failed",
            source.to_string()
        );
        assert!(
            std::error::Error::source(source.as_ref())
                .is_some_and(|error| error.to_string().contains("test failure"))
        );
    }

    #[test]
    fn should_map_complete_and_empty_prices_from_parts() {
        let price = match price_from_parts(Some(123), Some("EUR".to_owned())) {
            Ok(Some(price)) => price,
            Ok(None) => panic!("missing mapped price"),
            Err(error) => panic!("failed to map price: {error:?}"),
        };
        let empty = match price_from_parts(None, None) {
            Ok(value) => value,
            Err(error) => panic!("failed to map empty price: {error:?}"),
        };

        assert_eq!(MonetaryAmount::from(123_u64), price.monetary_amount);
        assert_eq!(Currency::Eur, price.currency);
        assert_eq!(None, empty);
    }

    #[test]
    fn should_reject_incomplete_negative_and_invalid_price_parts() {
        assert!(matches!(
            price_from_parts(Some(123), None),
            Err(ProductListingRepositoryError::IncompletePricePersisted)
        ));
        assert!(matches!(
            price_from_parts(Some(-1), Some("EUR".to_owned())),
            Err(ProductListingRepositoryError::NegativePriceAmountPersisted)
        ));
        assert!(matches!(
            price_from_parts(Some(123), Some("NOPE".to_owned())),
            Err(ProductListingRepositoryError::InvalidPriceCurrencyPersisted)
        ));
        assert!(matches!(
            price_from_parts(Some(123), Some("eur".to_owned())),
            Err(ProductListingRepositoryError::InvalidPriceCurrencyPersisted)
        ));
    }

    #[test]
    fn should_map_source_identity_language_and_image_branches() {
        let row = product_row();
        let title = match localized_title_from_row(&row) {
            Ok(Some(value)) => value,
            Ok(None) => panic!("missing title"),
            Err(error) => panic!("failed to map title: {error:?}"),
        };
        let description = match localized_description_from_row(&row) {
            Ok(Some(value)) => value,
            Ok(None) => panic!("missing description"),
            Err(error) => panic!("failed to map description: {error:?}"),
        };
        let images = match images_from_json(row.product_images.clone()) {
            Ok(value) => value,
            Err(error) => panic!("failed to map images: {error:?}"),
        };

        assert_eq!(Language::En, title.localization);
        assert_eq!(Language::De, description.localization);
        assert_eq!(1, images.len());
    }

    #[test]
    fn should_reject_incomplete_language_and_invalid_image_branches() {
        let mut row = product_row();
        row.title_language = None;
        assert!(matches!(
            localized_title_from_row(&row),
            Err(ProductListingRepositoryError::IncompleteTitlePersisted)
        ));

        row.title_language = Some("xx".to_owned());
        assert!(matches!(
            localized_title_from_row(&row),
            Err(ProductListingRepositoryError::InvalidTitleLanguagePersisted)
        ));

        let mut row = product_row();
        row.description_text = None;
        assert!(matches!(
            localized_description_from_row(&row),
            Err(ProductListingRepositoryError::IncompleteDescriptionPersisted)
        ));

        row.description_text = Some("description".to_owned());
        row.description_language = Some("xx".to_owned());
        assert!(matches!(
            localized_description_from_row(&row),
            Err(ProductListingRepositoryError::InvalidDescriptionLanguagePersisted)
        ));

        assert!(matches!(
            images_from_json(json!({"not": "array"})),
            Err(ProductListingRepositoryError::InvalidProductListingImagesPersisted)
        ));
        assert!(matches!(
            images_from_json(json!([{ "url": "not a url" }])),
            Err(ProductListingRepositoryError::InvalidProductListingImageUrlPersisted)
        ));
        assert!(matches!(
            images_from_json(json!([{ "url": "https://example.com/a.jpg", "extra": "BAD" }])),
            Err(ProductListingRepositoryError::InvalidProductListingImagesPersisted)
        ));
    }

    #[test]
    fn should_map_all_canonical_listing_enum_values() {
        for availability in ListingAvailability::iter() {
            assert_eq!(
                Some(availability),
                parse_availability(Some(availability.as_str()))
            );
        }
        assert_eq!(None, parse_availability(None));
        for lifecycle in ListingLifecycle::iter() {
            assert_eq!(lifecycle, parse_lifecycle(lifecycle.as_str()));
        }
    }

    #[test]
    fn should_reject_invalid_availability_lifecycle_and_product_row_values() {
        assert!(matches!(
            parse_listing_availability(Some("BAD")),
            Err(ProductListingRepositoryError::InvalidListingAvailabilityPersisted)
        ));
        assert!(matches!(
            parse_listing_lifecycle("BAD"),
            Err(ProductListingRepositoryError::InvalidListingLifecyclePersisted)
        ));

        let mut row = product_row();
        row.url = "http://[::1".to_owned();
        assert!(matches!(
            VersionedProductListing::try_from(row),
            Err(ProductListingRepositoryError::InvalidProductListingUrlPersisted)
        ));
    }

    fn parse_availability(value: Option<&str>) -> Option<ListingAvailability> {
        match parse_listing_availability(value) {
            Ok(availability) => availability,
            Err(error) => panic!("failed to parse listing availability: {error:?}"),
        }
    }

    fn parse_lifecycle(value: &str) -> ListingLifecycle {
        match parse_listing_lifecycle(value) {
            Ok(lifecycle) => lifecycle,
            Err(error) => panic!("failed to parse lifecycle: {error:?}"),
        }
    }

    fn product_row() -> ProductListingRow {
        let now = OffsetDateTime::now_utc();
        let title_slug = ProductListingSlugId::raw("unit-product-a1b2c3")
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}"))
            .as_ref()
            .to_owned();
        let source_listing_id = SourceListingId::try_from("unit-product")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
        ProductListingRow {
            product_listing_id: uuid::Uuid::new_v4(),
            product_listing_title_slug_id: title_slug,
            version: 1,
            current_event_id: uuid::Uuid::new_v4(),
            listing_source_id: uuid::Uuid::new_v4(),
            source_listing_id: source_listing_id.to_string(),
            title_text: Some("title".to_owned()),
            title_language: Some("en".to_owned()),
            description_text: Some("description".to_owned()),
            description_language: Some("de".to_owned()),
            price_amount: Some(1_200),
            price_currency: Some("EUR".to_owned()),
            price_estimate_min_amount: None,
            price_estimate_min_currency: None,
            price_estimate_max_amount: None,
            price_estimate_max_currency: None,
            sale_observation_fx_rate_id: None,
            sale_observed_at: None,
            availability: Some("AVAILABLE".to_owned()),
            lifecycle: "ACTIVE".to_owned(),
            url: "https://example.com/unit-product".to_owned(),
            product_images: json!([{ "url": "https://example.com/unit-product.jpg" }]),
            embedding: None,
            auction_start: None,
            auction_end: None,
            created: now,
            updated: now,
        }
    }
}

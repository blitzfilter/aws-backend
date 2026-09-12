#![allow(dead_code)]

use crate::{
    object_id::try_from_uuid,
    product_listing_auction::{
        ProductListingAuctionParts, auction_from_parts, auction_write_parts,
    },
};
use application::error::box_error;
use domain_primitives::event_id::EventId;
use domain_primitives::versioned::Versioned;

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
use product_listing_core::product_listing_price::ProductListingPrice;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

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
    auction_context_product_listing_id: Option<uuid::Uuid>,
    auction_lot_number: Option<String>,
    auction_catalogue_position: Option<i64>,
    auction_timing_product_listing_id: Option<uuid::Uuid>,
    auction_bidding_opens_precision: Option<String>,
    auction_bidding_opens_instant_at: Option<OffsetDateTime>,
    auction_bidding_opens_date_on: Option<time::Date>,
    auction_bidding_opens_source_timezone: Option<String>,
    auction_scheduled_closes_precision: Option<String>,
    auction_scheduled_closes_instant_at: Option<OffsetDateTime>,
    auction_scheduled_closes_date_on: Option<time::Date>,
    auction_scheduled_closes_source_timezone: Option<String>,
    auction_reported_closed_at: Option<OffsetDateTime>,
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
                product_listings.product_listing_id, product_listing_title_slug_id, version, current_event_id, listing_source_id, source_listing_id,
                title_text, title_language, description_text, description_language,
                price_kind, price_amount, price_currency, price_estimate_min_amount,
                price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_observation_fx_rate_id, sale_observed_at, availability, lifecycle, url,
                product_images, embedding,
                auction_context.product_listing_id AS auction_context_product_listing_id,
                auction_context.lot_number AS auction_lot_number,
                auction_context.catalogue_position AS auction_catalogue_position,
                auction_timing.product_listing_id AS auction_timing_product_listing_id,
                auction_timing.bidding_opens_precision AS auction_bidding_opens_precision,
                auction_timing.bidding_opens_instant_at AS auction_bidding_opens_instant_at,
                auction_timing.bidding_opens_date_on AS auction_bidding_opens_date_on,
                auction_timing.bidding_opens_source_timezone AS auction_bidding_opens_source_timezone,
                auction_timing.scheduled_closes_precision AS auction_scheduled_closes_precision,
                auction_timing.scheduled_closes_instant_at AS auction_scheduled_closes_instant_at,
                auction_timing.scheduled_closes_date_on AS auction_scheduled_closes_date_on,
                auction_timing.scheduled_closes_source_timezone AS auction_scheduled_closes_source_timezone,
                auction_timing.reported_closed_at AS auction_reported_closed_at,
                product_listings.created, product_listings.updated
            FROM product_listings
            LEFT JOIN product_listing_auction_contexts auction_context
                ON auction_context.product_listing_id = product_listings.product_listing_id
            LEFT JOIN product_listing_lot_auction_timings auction_timing
                ON auction_timing.product_listing_id = auction_context.product_listing_id
            WHERE product_listings.product_listing_id = $1
            "#,
        )
        .bind(id.as_uuid())
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
                product_listings.product_listing_id, product_listing_title_slug_id, version, current_event_id, listing_source_id, source_listing_id,
                title_text, title_language, description_text, description_language,
                price_kind, price_amount, price_currency, price_estimate_min_amount,
                price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_observation_fx_rate_id, sale_observed_at, availability, lifecycle, url,
                product_images, embedding,
                auction_context.product_listing_id AS auction_context_product_listing_id,
                auction_context.lot_number AS auction_lot_number,
                auction_context.catalogue_position AS auction_catalogue_position,
                auction_timing.product_listing_id AS auction_timing_product_listing_id,
                auction_timing.bidding_opens_precision AS auction_bidding_opens_precision,
                auction_timing.bidding_opens_instant_at AS auction_bidding_opens_instant_at,
                auction_timing.bidding_opens_date_on AS auction_bidding_opens_date_on,
                auction_timing.bidding_opens_source_timezone AS auction_bidding_opens_source_timezone,
                auction_timing.scheduled_closes_precision AS auction_scheduled_closes_precision,
                auction_timing.scheduled_closes_instant_at AS auction_scheduled_closes_instant_at,
                auction_timing.scheduled_closes_date_on AS auction_scheduled_closes_date_on,
                auction_timing.scheduled_closes_source_timezone AS auction_scheduled_closes_source_timezone,
                auction_timing.reported_closed_at AS auction_reported_closed_at,
                product_listings.created, product_listings.updated
            FROM product_listings
            LEFT JOIN product_listing_auction_contexts auction_context
                ON auction_context.product_listing_id = product_listings.product_listing_id
            LEFT JOIN product_listing_lot_auction_timings auction_timing
                ON auction_timing.product_listing_id = auction_context.product_listing_id
            WHERE product_listings.listing_source_id = $1
              AND product_listings.source_listing_id = $2
            "#,
        )
        .bind(key.listing_source_id.as_uuid())
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
        let title = product.title();
        let description = product.description();
        let (price_kind, price_amount, price_currency) =
            product_listing_price_to_parts(pricing.price)
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
                description_text, description_language, price_kind, price_amount, price_currency,
                price_estimate_min_amount, price_estimate_min_currency, price_estimate_max_amount,
                price_estimate_max_currency, sale_observation_fx_rate_id, sale_observed_at,
                availability, lifecycle, url, product_images
            ) VALUES (
                $1, $2, $3, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15, $16, $17, $18, $19, $20, $21, $22
            )
            RETURNING version
            "#,
        )
        .bind(product.id().as_uuid())
        .bind(product.title_slug_id().as_ref().to_owned())
        .bind(current_event_id.as_uuid())
        .bind(product.listing_source_id().as_uuid())
        .bind(product.source_listing_id().as_ref().to_owned())
        .bind(title.map(|value| value.payload.as_ref().to_owned()))
        .bind(title.map(|value| value.localization.as_str().to_owned()))
        .bind(description.map(|value| value.payload.as_ref().to_owned()))
        .bind(description.map(|value| value.localization.as_str().to_owned()))
        .bind(price_kind)
        .bind(price_amount)
        .bind(price_currency)
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
                .map(|value| value.fx_rate_id().into_uuid()),
        )
        .bind(product.sale_observation().map(|value| value.observed_at()))
        .bind(product.availability().map(ListingAvailability::as_str))
        .bind(product.lifecycle().as_str())
        .bind(product.url().to_string())
        .bind(product_images)
        .fetch_one(&mut *self.connection)
        .await
        .map_err(ProductListingInsertSqlxError)?;

        self.replace_auction_context(product.id(), product.auction())
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
        let title = product.title();
        let description = product.description();
        let (price_kind, price_amount, price_currency) =
            product_listing_price_to_parts(pricing.price)
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
                price_kind = $7,
                price_amount = $8,
                price_currency = $9,
                price_estimate_min_amount = $10,
                price_estimate_min_currency = $11,
                price_estimate_max_amount = $12,
                price_estimate_max_currency = $13,
                sale_observation_fx_rate_id = $14,
                sale_observed_at = $15,
                availability = $16,
                lifecycle = $17,
                url = $18,
                product_images = $19,
                version = version + 1,
                projection_version = projection_version + 1,
                updated = now()
            WHERE product_listing_id = $20 AND version = $21
            RETURNING version
            "#,
        )
        .bind(current_event_id.as_uuid())
        .bind(effects.advance_embedding_source)
        .bind(title.map(|value| value.payload.as_ref().to_owned()))
        .bind(title.map(|value| value.localization.as_str().to_owned()))
        .bind(description.map(|value| value.payload.as_ref().to_owned()))
        .bind(description.map(|value| value.localization.as_str().to_owned()))
        .bind(price_kind)
        .bind(price_amount)
        .bind(price_currency)
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
                .map(|value| value.fx_rate_id().into_uuid()),
        )
        .bind(product.sale_observation().map(|value| value.observed_at()))
        .bind(product.availability().map(ListingAvailability::as_str))
        .bind(product.lifecycle().as_str())
        .bind(product.url().to_string())
        .bind(product_images)
        .bind(product.id().as_uuid())
        .bind(expected_version)
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingUpdateSqlxError)?
        .ok_or(ProductListingRepositoryError::ConcurrencyConflict)?;

        self.replace_auction_context(product.id(), product.auction())
            .await
            .map_err(ProductListingUpdateSqlxError)?;

        let version = ProductListingStorageVersion::try_from(version)
            .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?;
        Ok(Versioned::new(product.clone(), version))
    }
}

impl SqlxProductListingRepository<'_> {
    async fn replace_auction_context(
        &mut self,
        product_listing_id: ProductListingId,
        auction: Option<&ProductListingAuction>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM product_listing_auction_contexts WHERE product_listing_id = $1")
            .bind(product_listing_id.as_uuid())
            .execute(&mut *self.connection)
            .await?;

        let Some(auction) = auction else {
            return Ok(());
        };
        let parts = auction_write_parts(auction);
        sqlx::query(
            r#"
            INSERT INTO product_listing_auction_contexts (
                product_listing_id, lot_number, catalogue_position
            ) VALUES ($1, $2, $3)
            "#,
        )
        .bind(product_listing_id.as_uuid())
        .bind(parts.lot_number)
        .bind(parts.catalogue_position)
        .execute(&mut *self.connection)
        .await?;

        let Some(timing) = parts.timing else {
            return Ok(());
        };
        sqlx::query(
            r#"
            INSERT INTO product_listing_lot_auction_timings (
                product_listing_id,
                bidding_opens_precision, bidding_opens_instant_at, bidding_opens_date_on,
                bidding_opens_source_timezone,
                scheduled_closes_precision, scheduled_closes_instant_at, scheduled_closes_date_on,
                scheduled_closes_source_timezone, reported_closed_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(product_listing_id.as_uuid())
        .bind(timing.bidding_opens.precision)
        .bind(timing.bidding_opens.instant_at)
        .bind(timing.bidding_opens.date_on)
        .bind(timing.bidding_opens.source_timezone)
        .bind(timing.scheduled_closes.precision)
        .bind(timing.scheduled_closes.instant_at)
        .bind(timing.scheduled_closes.date_on)
        .bind(timing.scheduled_closes.source_timezone)
        .bind(timing.reported_closed_at)
        .execute(&mut *self.connection)
        .await?;
        Ok(())
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
        let auction = auction_from_parts(ProductListingAuctionParts {
            context_product_listing_id: row.auction_context_product_listing_id,
            lot_number: row.auction_lot_number,
            catalogue_position: row.auction_catalogue_position,
            timing_product_listing_id: row.auction_timing_product_listing_id,
            bidding_opens_precision: row.auction_bidding_opens_precision,
            bidding_opens_instant_at: row.auction_bidding_opens_instant_at,
            bidding_opens_date_on: row.auction_bidding_opens_date_on,
            bidding_opens_source_timezone: row.auction_bidding_opens_source_timezone,
            scheduled_closes_precision: row.auction_scheduled_closes_precision,
            scheduled_closes_instant_at: row.auction_scheduled_closes_instant_at,
            scheduled_closes_date_on: row.auction_scheduled_closes_date_on,
            scheduled_closes_source_timezone: row.auction_scheduled_closes_source_timezone,
            reported_closed_at: row.auction_reported_closed_at,
        })
        .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?;
        let product = ProductListing::rehydrate(RehydratedProductListingState {
            id: try_from_uuid(row.product_listing_id, "ProductListing ID")
                .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?,
            title_slug_id: ProductListingSlugId::raw(&row.product_listing_title_slug_id)
                .map_err(|_| ProductListingRepositoryError::InvalidProductListingSlugPersisted)?,
            listing_source_id: try_from_uuid(row.listing_source_id, "ListingSource ID")
                .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?,
            source_listing_id,
            title,
            description,
            pricing: ProductListingPricing {
                price: product_listing_price_from_parts(
                    row.price_kind,
                    row.price_amount,
                    row.price_currency,
                )?,
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
            auction,
        })
        .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?;

        let _current_event_id = try_from_uuid::<EventId>(row.current_event_id, "current event ID")
            .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?;
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
            try_from_uuid(fx_rate_id, "sale observation FX rate ID")
                .map_err(|_| ProductListingRepositoryError::InvalidAggregateStatePersisted)?,
        ))),
        (None, None) => Ok(None),
        _ => Err(ProductListingRepositoryError::InvalidAggregateStatePersisted),
    }
}

fn amount_to_i64(amount: MonetaryAmount) -> Result<i64, ()> {
    i64::try_from(u64::from(amount)).map_err(|_| ())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductListingPriceKind {
    Monetary,
    OnRequest,
}

impl ProductListingPriceKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Monetary => "MONETARY",
            Self::OnRequest => "ON_REQUEST",
        }
    }

    fn parse(value: &str) -> Result<Self, ProductListingRepositoryError> {
        match value {
            "MONETARY" => Ok(Self::Monetary),
            "ON_REQUEST" => Ok(Self::OnRequest),
            _ => Err(ProductListingRepositoryError::InvalidProductListingPriceKindPersisted),
        }
    }
}

#[allow(clippy::type_complexity)]
fn product_listing_price_to_parts(
    price: Option<ProductListingPrice>,
) -> Result<(Option<&'static str>, Option<i64>, Option<String>), ()> {
    match price {
        None => Ok((None, None, None)),
        Some(ProductListingPrice::Monetary(price)) => Ok((
            Some(ProductListingPriceKind::Monetary.as_str()),
            Some(amount_to_i64(price.monetary_amount)?),
            Some(price.currency.as_str().to_owned()),
        )),
        Some(ProductListingPrice::OnRequest) => Ok((
            Some(ProductListingPriceKind::OnRequest.as_str()),
            None,
            None,
        )),
    }
}

fn product_listing_price_from_parts(
    kind: Option<String>,
    amount: Option<i64>,
    currency: Option<String>,
) -> Result<Option<ProductListingPrice>, ProductListingRepositoryError> {
    let Some(kind) = kind else {
        return match (amount, currency) {
            (None, None) => Ok(None),
            _ => Err(ProductListingRepositoryError::InvalidProductListingPricePersisted),
        };
    };

    match (ProductListingPriceKind::parse(&kind)?, amount, currency) {
        (ProductListingPriceKind::Monetary, Some(amount), Some(currency)) => {
            let amount = u64::try_from(amount)
                .map_err(|_| ProductListingRepositoryError::NegativePriceAmountPersisted)?;
            Ok(Some(ProductListingPrice::Monetary(Price::new(
                MonetaryAmount::from(amount),
                parse_currency(&currency)?,
            ))))
        }
        (ProductListingPriceKind::OnRequest, None, None) => {
            Ok(Some(ProductListingPrice::OnRequest))
        }
        _ => Err(ProductListingRepositoryError::InvalidProductListingPricePersisted),
    }
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

    use listing_source_core::ListingSourceId;
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
    fn should_map_all_product_listing_price_states_from_parts() {
        assert!(matches!(
            product_listing_price_from_parts(None, None, None),
            Ok(None)
        ));
        assert!(matches!(
            product_listing_price_from_parts(
                Some("MONETARY".to_owned()),
                Some(123),
                Some("EUR".to_owned()),
            ),
            Ok(Some(ProductListingPrice::Monetary(price)))
                if price == Price::new(MonetaryAmount::from(123_u64), Currency::Eur)
        ));
        assert!(matches!(
            product_listing_price_from_parts(Some("ON_REQUEST".to_owned()), None, None),
            Ok(Some(ProductListingPrice::OnRequest))
        ));
    }

    #[test]
    fn should_reject_corrupt_product_listing_price_parts() {
        for parts in [
            (None, Some(1), None),
            (None, None, Some("EUR".to_owned())),
            (Some("MONETARY".to_owned()), None, Some("EUR".to_owned())),
            (Some("MONETARY".to_owned()), Some(1), None),
            (Some("ON_REQUEST".to_owned()), Some(1), None),
            (Some("ON_REQUEST".to_owned()), None, Some("EUR".to_owned())),
            (Some("UNKNOWN".to_owned()), None, None),
        ] {
            assert!(product_listing_price_from_parts(parts.0, parts.1, parts.2).is_err());
        }
        assert!(matches!(
            product_listing_price_from_parts(
                Some("MONETARY".to_owned()),
                Some(-1),
                Some("EUR".to_owned()),
            ),
            Err(ProductListingRepositoryError::NegativePriceAmountPersisted)
        ));
        assert!(matches!(
            product_listing_price_from_parts(
                Some("MONETARY".to_owned()),
                Some(1),
                Some("NOPE".to_owned()),
            ),
            Err(ProductListingRepositoryError::InvalidPriceCurrencyPersisted)
        ));
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
            product_listing_id: ProductListingId::new().into_uuid(),
            product_listing_title_slug_id: title_slug,
            version: 1,
            current_event_id: EventId::new().into_uuid(),
            listing_source_id: ListingSourceId::new().into_uuid(),
            source_listing_id: source_listing_id.to_string(),
            title_text: Some("title".to_owned()),
            title_language: Some("en".to_owned()),
            description_text: Some("description".to_owned()),
            description_language: Some("de".to_owned()),
            price_kind: Some("MONETARY".to_owned()),
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
            auction_context_product_listing_id: None,
            auction_lot_number: None,
            auction_catalogue_position: None,
            auction_timing_product_listing_id: None,
            auction_bidding_opens_precision: None,
            auction_bidding_opens_instant_at: None,
            auction_bidding_opens_date_on: None,
            auction_bidding_opens_source_timezone: None,
            auction_scheduled_closes_precision: None,
            auction_scheduled_closes_instant_at: None,
            auction_scheduled_closes_date_on: None,
            auction_scheduled_closes_source_timezone: None,
            auction_reported_closed_at: None,
            created: now,
            updated: now,
        }
    }
}

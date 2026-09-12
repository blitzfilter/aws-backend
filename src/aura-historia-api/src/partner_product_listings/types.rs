use crate::error::{ApiError, ApiErrorCode, BAD_BODY_VALUE};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use crate::wire::parse_path_object_id;
use application::patch_field::PatchField;
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionName, AuctionReportedStatus, AuctionTime,
    AuctionTimeZone, ReportedCatalogueLotCount, SourceAuctionId,
};
use auction_service::EmbeddedAuctionMetadata;
use listing_source_core::ListingSourceId;
use money::Price;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    CataloguePosition, LotAuctionTiming, LotNumber, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use product_listing_service::use_cases::{
    CreateProductListingCommand, UpdateProductListingCommand, UpsertProductListingCommand,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use time::{Date, OffsetDateTime, format_description::well_known::Iso8601};
use url::Url;

pub(super) const MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateProductListingData {
    pub(super) source_listing_id: String,
    pub(super) title: LocalizedTextData,
    pub(super) description: LocalizedTextData,
    #[serde(default)]
    pub(super) price: Option<ProductListingPriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: Option<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: Option<PriceData>,
    #[serde(default, with = "crate::wire::listing_availability::option")]
    pub(super) availability: Option<ListingAvailability>,
    pub(super) url: Url,
    pub(super) images: Vec<Url>,
    #[serde(default)]
    auction: Option<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateProductListingData {
    pub(super) source_listing_id: String,
    #[serde(default)]
    pub(super) price: PatchValue<ProductListingPriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: PatchValue<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: PatchValue<PriceData>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::listing_availability::patch::deserialize")]
    pub(super) availability: PatchValue<ListingAvailability>,
    #[serde(default)]
    pub(super) url: PatchValue<Url>,
    #[serde(default)]
    pub(super) images: PatchValue<Vec<Url>>,
    #[serde(default)]
    auction: PatchValue<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpsertProductListingData {
    pub(super) source_listing_id: String,
    #[serde(default)]
    pub(super) title: Option<LocalizedTextData>,
    #[serde(default)]
    pub(super) description: Option<LocalizedTextData>,
    #[serde(default)]
    pub(super) price: PatchValue<ProductListingPriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: PatchValue<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: PatchValue<PriceData>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::listing_availability::patch::deserialize")]
    pub(super) availability: PatchValue<ListingAvailability>,
    #[serde(default)]
    pub(super) url: Option<Url>,
    #[serde(default)]
    pub(super) images: PatchValue<Vec<Url>>,
    #[serde(default)]
    auction: PatchValue<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProductListingAuctionData {
    #[serde(default)]
    source_auction_id: Option<String>,
    #[serde(default)]
    metadata: EmbeddedAuctionMetadataData,
    #[serde(default)]
    lot_number: Option<String>,
    #[serde(default)]
    catalogue_position: Option<u64>,
    #[serde(default)]
    timing: Option<LotAuctionTimingData>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedAuctionMetadataData {
    #[serde(default)]
    name: Option<LocalizedTextData>,
    #[serde(default)]
    description: Option<LocalizedTextData>,
    #[serde(default)]
    catalogue_url: Option<Url>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    reported_status: Option<String>,
    #[serde(default)]
    reported_lot_count: Option<u32>,
    #[serde(default)]
    schedule: EmbeddedAuctionScheduleData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedAuctionScheduleData {
    #[serde(default)]
    bidding_opens: Option<AuctionTimeData>,
    #[serde(default)]
    live_starts: Option<AuctionTimeData>,
    #[serde(default)]
    lots_begin_closing: Option<AuctionTimeData>,
    #[serde(default)]
    scheduled_end: Option<AuctionTimeData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LotAuctionTimingData {
    #[serde(default)]
    bidding_opens: Option<AuctionTimeData>,
    #[serde(default)]
    scheduled_closes: Option<AuctionTimeData>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    reported_closed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "precision",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum AuctionTimeData {
    Instant {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        #[serde(default, rename = "sourceTimezone")]
        source_timezone: Option<String>,
    },
    Date {
        on: String,
        #[serde(default, rename = "sourceTimezone")]
        source_timezone: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WithdrawProductListingData {
    pub(super) source_listing_id: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PartnerProductFailureData {
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    error: ApiErrorCode,
}

pub(super) fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ApiError> {
    parse_path_object_id(value, "listingSourceId", "ListingSource")
}

pub(super) fn parse_partner_product_batch<T: DeserializeOwned>(
    body: &str,
) -> Result<Vec<T>, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }

    let products: Vec<T> = serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))?;
    if products.len() > MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail(format!(
            "Body cannot contain more than {MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE} products."
        )));
    }

    Ok(products)
}

impl CreateProductListingData {
    pub(super) fn into_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<CreateProductListingCommand, ApiError> {
        let auction = self
            .auction
            .map(ProductListingAuctionData::into_core)
            .transpose()?;
        Ok(CreateProductListingCommand {
            listing_source_id,
            source_listing_id: source_listing_id(self.source_listing_id)?,
            title: Some(title(self.title)),
            description: Some(description(self.description)),
            pricing: ProductListingPricing {
                price: self.price.map(product_listing_price),
                price_estimate_min: self.price_estimate_min.map(price),
                price_estimate_max: self.price_estimate_max.map(price),
            },
            availability: self.availability,
            url: self.url,
            images: product_images(self.images),
            auction: auction.as_ref().map(|value| value.context.clone()),
            auction_source_id: auction
                .as_ref()
                .and_then(|value| value.source_auction_id.clone()),
            auction_metadata: auction
                .map_or_else(EmbeddedAuctionMetadata::default, |value| value.metadata),
        })
    }
}

impl UpdateProductListingData {
    pub(super) fn into_key_and_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<(ProductListingKey, UpdateProductListingCommand), ApiError> {
        let product_key = ProductListingKey::new(
            listing_source_id,
            source_listing_id(self.source_listing_id)?,
        );
        let auction = auction_patch(self.auction)?;
        let command = UpdateProductListingCommand {
            price: clearable(self.price.map(product_listing_price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: non_nullable_patch(self.url, "url")?,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction: auction.auction,
            auction_source_id: auction.source_auction_id,
            auction_metadata: auction.metadata,
        };
        Ok((product_key, command))
    }
}

impl UpsertProductListingData {
    pub(super) fn into_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<UpsertProductListingCommand, ApiError> {
        let auction = auction_patch(self.auction)?;
        Ok(UpsertProductListingCommand {
            listing_source_id,
            source_listing_id: source_listing_id(self.source_listing_id)?,
            title: self.title.map(title),
            description: self.description.map(description),
            price: clearable(self.price.map(product_listing_price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: self.url,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction: auction.auction,
            auction_source_id: auction.source_auction_id,
            auction_metadata: auction.metadata,
        })
    }
}

struct ProductListingAuctionContext {
    context: ProductListingAuction,
    source_auction_id: Option<SourceAuctionId>,
    metadata: EmbeddedAuctionMetadata,
}

struct ProductListingAuctionPatch {
    auction: PatchField<ProductListingAuction>,
    source_auction_id: Option<SourceAuctionId>,
    metadata: EmbeddedAuctionMetadata,
}

impl ProductListingAuctionData {
    fn into_core(self) -> Result<ProductListingAuctionContext, ApiError> {
        let lot_number = self
            .lot_number
            .map(|value| {
                LotNumber::try_from(value).map_err(|_| {
                    ApiError::bad_request(BAD_BODY_VALUE)
                        .with_detail("auction.lotNumber must be nonblank, NUL-free, and at most 128 UTF-8 bytes.")
                })
            })
            .transpose()?;
        let catalogue_position = self
            .catalogue_position
            .map(|value| {
                CataloguePosition::try_from(value).map_err(|_| {
                    ApiError::bad_request(BAD_BODY_VALUE)
                        .with_detail("auction.cataloguePosition must be a positive 32-bit integer.")
                })
            })
            .transpose()?;
        let timing = self
            .timing
            .map(LotAuctionTimingData::into_core)
            .transpose()?;

        Ok(ProductListingAuctionContext {
            context: ProductListingAuction::new(None, lot_number, catalogue_position, timing),
            source_auction_id: self.source_auction_id.map(source_auction_id).transpose()?,
            metadata: self.metadata.into_core()?,
        })
    }
}

impl EmbeddedAuctionMetadataData {
    fn into_core(self) -> Result<EmbeddedAuctionMetadata, ApiError> {
        Ok(EmbeddedAuctionMetadata {
            name: self.name.map(auction_name).transpose()?,
            description: self.description.map(auction_description).transpose()?,
            catalogue_url: self.catalogue_url,
            format: self.format.map(auction_format).transpose()?,
            reported_status: self.reported_status.map(auction_status).transpose()?,
            reported_lot_count: self.reported_lot_count.map(ReportedCatalogueLotCount::new),
            ..self.schedule.into_core()?
        })
    }
}

impl EmbeddedAuctionScheduleData {
    fn into_core(self) -> Result<EmbeddedAuctionMetadata, ApiError> {
        Ok(EmbeddedAuctionMetadata {
            bidding_opens: self
                .bidding_opens
                .map(AuctionTimeData::into_core)
                .transpose()?,
            live_starts: self
                .live_starts
                .map(AuctionTimeData::into_core)
                .transpose()?,
            lots_begin_closing: self
                .lots_begin_closing
                .map(AuctionTimeData::into_core)
                .transpose()?,
            scheduled_end: self
                .scheduled_end
                .map(AuctionTimeData::into_core)
                .transpose()?,
            ..EmbeddedAuctionMetadata::default()
        })
    }
}

impl LotAuctionTimingData {
    fn into_core(self) -> Result<LotAuctionTiming, ApiError> {
        LotAuctionTiming::new(
            self.bidding_opens
                .map(AuctionTimeData::into_core)
                .transpose()?,
            self.scheduled_closes
                .map(AuctionTimeData::into_core)
                .transpose()?,
            self.reported_closed_at,
        )
        .map_err(|_| {
            ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail("auction.timing has invalid comparable bounds.")
        })
    }
}

impl AuctionTimeData {
    fn into_core(self) -> Result<AuctionTime, ApiError> {
        match self {
            Self::Instant {
                at,
                source_timezone,
            } => Ok(AuctionTime::instant(
                at,
                source_timezone.map(timezone).transpose()?,
            )),
            Self::Date {
                on,
                source_timezone,
            } => {
                let on = Date::parse(&on, &Iso8601::DATE).map_err(|_| {
                    ApiError::bad_request(BAD_BODY_VALUE)
                        .with_detail("auction timing date must use YYYY-MM-DD.")
                })?;
                Ok(AuctionTime::date(
                    on,
                    source_timezone.map(timezone).transpose()?,
                ))
            }
        }
    }
}

impl WithdrawProductListingData {
    pub(super) fn into_product_key(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<ProductListingKey, ApiError> {
        Ok(ProductListingKey::new(
            listing_source_id,
            source_listing_id(self.source_listing_id)?,
        ))
    }
}

impl PartnerProductFailureData {
    pub(super) fn new(
        listing_source_id: ListingSourceId,
        source_listing_id: String,
        error: ApiErrorCode,
    ) -> Self {
        Self {
            listing_source_id,
            source_listing_id,
            error,
        }
    }
}

fn title(value: LocalizedTextData) -> localization::Localized<localization::Language, Title> {
    value.into_localized()
}

fn description(
    value: LocalizedTextData,
) -> localization::Localized<localization::Language, Description> {
    value.into_localized()
}

fn price(value: PriceData) -> Price {
    value.into()
}

fn product_listing_price(
    value: ProductListingPriceData,
) -> product_listing_core::product_listing_price::ProductListingPrice {
    value.into()
}

fn product_images(values: Vec<Url>) -> indexmap::IndexSet<ProductListingImage> {
    values.into_iter().map(ProductListingImage::new).collect()
}

fn auction_patch(
    value: PatchValue<ProductListingAuctionData>,
) -> Result<ProductListingAuctionPatch, ApiError> {
    match value {
        PatchValue::Omitted => Ok(ProductListingAuctionPatch {
            auction: PatchField::Unchanged,
            source_auction_id: None,
            metadata: EmbeddedAuctionMetadata::default(),
        }),
        PatchValue::Null => Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail(
            "auction cannot be null in an ordinary update; use the dedicated correction operation.",
        )),
        PatchValue::Value(value) => {
            let value = value.into_core()?;
            Ok(ProductListingAuctionPatch {
                auction: PatchField::Set(value.context),
                source_auction_id: value.source_auction_id,
                metadata: value.metadata,
            })
        }
    }
}

fn auction_name(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, AuctionName>, ApiError> {
    AuctionName::try_from(value.text)
        .map(|payload| localization::Localized::new(value.language, payload))
        .map_err(|_| ApiError::bad_request(BAD_BODY_VALUE).with_detail("auction.metadata.name.text must be nonblank, NUL-free, and at most 512 UTF-8 bytes."))
}

fn auction_description(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, AuctionDescription>, ApiError> {
    AuctionDescription::try_from(value.text)
        .map(|payload| localization::Localized::new(value.language, payload))
        .map_err(|_| ApiError::bad_request(BAD_BODY_VALUE).with_detail("auction.metadata.description.text must be valid nonblank sanitized text of at most 65536 UTF-8 bytes."))
}

fn auction_format(value: String) -> Result<AuctionFormat, ApiError> {
    value.parse().map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("auction.metadata.format must be LIVE or TIMED.")
    })
}

fn auction_status(value: String) -> Result<AuctionReportedStatus, ApiError> {
    value.parse().map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("auction.metadata.reportedStatus must be SCHEDULED, IN_PROGRESS, ENDED, POSTPONED, or CANCELLED.")
    })
}

fn source_auction_id(value: String) -> Result<SourceAuctionId, ApiError> {
    SourceAuctionId::try_from(value).map_err(|error| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail(format!("auction.sourceAuctionId is invalid: {error}"))
    })
}

fn timezone(value: String) -> Result<AuctionTimeZone, ApiError> {
    AuctionTimeZone::try_from(value).map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("auction timing sourceTimezone must be a valid IANA timezone identifier.")
    })
}

fn source_listing_id(value: String) -> Result<SourceListingId, ApiError> {
    SourceListingId::try_from(value)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        CreateProductListingData, UpdateProductListingData, UpsertProductListingData,
        WithdrawProductListingData, parse_listing_source_id, source_listing_id,
    };
    use crate::error::{BAD_BODY_VALUE, INVALID_OBJECT_ID};
    use application::patch_field::PatchField;
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::ProductListingId;

    #[test]
    fn should_reject_null_auction_in_ordinary_partner_updates() {
        let listing_source_id = ListingSourceId::new();
        let update: UpdateProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":null}"#)
                .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
        let upsert: UpsertProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":null}"#)
                .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

        assert_eq!(
            BAD_BODY_VALUE,
            update
                .into_key_and_command(listing_source_id)
                .err()
                .unwrap_or_else(|| panic!("null auction must fail"))
                .code()
        );
        assert_eq!(
            BAD_BODY_VALUE,
            upsert
                .into_command(listing_source_id)
                .err()
                .unwrap_or_else(|| panic!("null auction must fail"))
                .code()
        );
    }

    #[test]
    fn should_map_reliable_auction_key_and_embedded_metadata_for_partner_create() {
        let data: CreateProductListingData = serde_json::from_str(
            r#"{
                "sourceListingId":"SKU-1",
                "title":{"text":"Listing","language":"en"},
                "description":{"text":"Description","language":"en"},
                "url":"https://example.com/listing",
                "images":[],
                "auction":{
                    "sourceAuctionId":" sale-42 ",
                    "metadata":{
                        "name":{"text":"Spring sale","language":"en"},
                        "format":"TIMED",
                        "reportedLotCount":100,
                        "schedule":{"scheduledEnd":{"precision":"INSTANT","at":"2026-05-01T12:00:00Z"}}
                    },
                    "lotNumber":"42"
                }
            }"#,
        )
        .unwrap_or_else(|error| panic!("valid create JSON: {error}"));

        let command = data
            .into_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid create command: {error}"));

        assert_eq!(
            command.auction_source_id.as_ref().map(AsRef::as_ref),
            Some("sale-42")
        );
        assert_eq!(
            command
                .auction_metadata
                .name
                .as_ref()
                .map(|value| value.payload.as_ref()),
            Some("Spring sale")
        );
        assert!(command.auction_metadata.scheduled_end.is_some());
        assert!(command.auction.is_some());
    }

    #[test]
    fn should_reject_invalid_reliable_auction_key_and_unknown_metadata_fields() {
        let invalid_key: UpdateProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","auction":{"sourceAuctionId":" \t"}}"#,
        )
        .unwrap_or_else(|error| panic!("valid JSON shape: {error}"));
        let error = invalid_key
            .into_key_and_command(ListingSourceId::new())
            .err()
            .unwrap_or_else(|| panic!("blank source auction ID must fail"));
        assert_eq!(error.code(), BAD_BODY_VALUE);

        let unknown_metadata = serde_json::from_str::<UpdateProductListingData>(
            r#"{"sourceListingId":"SKU-1","auction":{"metadata":{"unknown":true}}}"#,
        );
        assert!(unknown_metadata.is_err());
    }

    #[test]
    fn should_leave_auction_unchanged_when_partner_update_or_upsert_omits_it() {
        let update: UpdateProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1"}"#)
                .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
        let upsert: UpsertProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1"}"#)
                .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

        let (_, update) = update
            .into_key_and_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid update command: {error}"));
        let upsert = upsert
            .into_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid upsert command: {error}"));

        assert_eq!(update.auction, PatchField::Unchanged);
        assert!(update.auction_source_id.is_none());
        assert_eq!(upsert.auction, PatchField::Unchanged);
        assert!(upsert.auction_source_id.is_none());
    }

    #[test]
    fn should_parse_source_listing_id_without_slugifying_it() {
        let data: WithdrawProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"\u2003SKU  #42/Blue\u2002"}"#)
                .unwrap_or_else(|error| panic!("valid request data: {error}"));

        let key = data
            .into_product_key(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));

        assert_eq!(key.source_listing_id.as_ref(), "SKU  #42/Blue");
    }

    #[test]
    fn should_parse_only_canonical_listing_source_object_ids() {
        let listing_source_id = ListingSourceId::new();
        assert!(matches!(
            parse_listing_source_id(&listing_source_id.to_string()),
            Ok(parsed) if parsed == listing_source_id
        ));

        for invalid_id in [
            ProductListingId::new().to_string(),
            listing_source_id.as_uuid().to_string(),
            "ls_not-a-typeid".to_owned(),
        ] {
            let error = parse_listing_source_id(&invalid_id)
                .err()
                .unwrap_or_else(|| panic!("noncanonical ListingSource ID was accepted"));
            assert_eq!(INVALID_OBJECT_ID, error.code());
        }
    }

    #[test]
    fn should_reject_blank_source_listing_id_at_api_mapping() {
        let error = source_listing_id("\u{2003}\t".to_owned())
            .err()
            .unwrap_or_else(|| panic!("blank source listing ID must fail"));

        assert_eq!(error.code(), BAD_BODY_VALUE);
    }
}

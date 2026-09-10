use crate::error::{ApiError, ApiErrorCode, BAD_BODY_VALUE};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use crate::wire::parse_path_object_id;

use listing_source_core::ListingSourceId;
use money::Price;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{ProductListingAuction, ProductListingPricing};
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use product_listing_service::use_cases::{
    CreateProductListingCommand, UpdateProductListingCommand, UpsertProductListingCommand,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use time::OffsetDateTime;
use url::Url;

pub(super) const MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(super) auction_start: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(super) auction_end: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    pub(super) auction_start: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    pub(super) auction_end: PatchValue<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    pub(super) auction_start: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    pub(super) auction_end: PatchValue<OffsetDateTime>,
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
            auction: ProductListingAuction {
                start: self.auction_start,
                end: self.auction_end,
            },
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
        let command = UpdateProductListingCommand {
            price: clearable(self.price.map(product_listing_price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: non_nullable_patch(self.url, "url")?,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction_start: clearable(self.auction_start.map(Some)),
            auction_end: clearable(self.auction_end.map(Some)),
        };
        Ok((product_key, command))
    }
}

impl UpsertProductListingData {
    pub(super) fn into_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<UpsertProductListingCommand, ApiError> {
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
            auction_start: clearable(self.auction_start),
            auction_end: clearable(self.auction_end),
        })
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

fn source_listing_id(value: String) -> Result<SourceListingId, ApiError> {
    SourceListingId::try_from(value)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{WithdrawProductListingData, parse_listing_source_id, source_listing_id};
    use crate::error::{BAD_BODY_VALUE, INVALID_OBJECT_ID};
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::ProductListingId;

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

//! Crawler-owned mapping from selected CSS extraction output to the generic raw-input contract.

use crate::scraper::auction::CrawlerAuctionEvidence;
use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use money::Currency;
use product_listing_normalization::{
    NormalizationContext, NormalizationInputError, ProductListingNormalizationInput,
    ProductListingRawValues, ProductListingRawValuesAuction,
    ProductListingRawValuesAuctionMetadata, ProductListingRawValuesPatch,
    ProductListingRawValuesPriceFormat, RawProductListingOperation, RawProductListingPayloadFormat,
    RawProductListingProvenance, RawProductListingValues, SourcePayload,
};
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

/// Builds the complete current raw input retained by the operational raw-revision stream.
///
/// Source payload keeps the untouched extraction. Generic raw values use the
/// crawler-validated image projection and omit price fields that deterministic
/// preparation could not resolve; the worker remains the sole canonical
/// normalization authority.
pub(crate) fn crawler_raw_input(
    raw: &RawExtractedProduct,
    validated_image_urls: &[String],
    candidate_url: &Url,
    auction: Option<&CrawlerAuctionEvidence>,
    fallback_currency: Option<Currency>,
    resolved_price_fields: [bool; 3],
) -> Result<ProductListingNormalizationInput, NormalizationInputError> {
    let source_payload = serde_json::to_value(raw)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(SourcePayload::new)?;
    let attributes = raw
        .raw_attributes
        .iter()
        .map(|(key, values)| {
            (
                key.clone(),
                ProductListingRawValuesPatch::Set(values.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let raw_values = ProductListingRawValues {
        source_listing_id: raw.source_listing_id.clone(),
        title: ProductListingRawValuesPatch::Set(raw.title.clone()),
        description: ProductListingRawValuesPatch::Set(raw.description.clone()),
        price_format: ProductListingRawValuesPriceFormat::DisplayText,
        price: price_patch(raw.price.clone(), resolved_price_fields[0]),
        price_estimate_min: price_patch(raw.price_estimate_min.clone(), resolved_price_fields[1]),
        price_estimate_max: price_patch(raw.price_estimate_max.clone(), resolved_price_fields[2]),
        availability: ProductListingRawValuesPatch::Set(raw.state.clone()),
        url: ProductListingRawValuesPatch::Set(candidate_url.to_string()),
        images: ProductListingRawValuesPatch::Set(validated_image_urls.to_vec()),
        auction: auction.map_or(ProductListingRawValuesPatch::Unchanged, |auction| {
            ProductListingRawValuesPatch::Set(ProductListingRawValuesAuction {
                source_auction_id: ProductListingRawValuesPatch::Set(
                    auction.source_auction_id.clone(),
                ),
                lot_number: auction.lot_number.clone(),
                catalogue_position: None,
                timing: None,
                auction_metadata: ProductListingRawValuesAuctionMetadata {
                    name: auction.name.clone(),
                    description: None,
                    catalogue_url: Some(auction.catalogue_url.clone()),
                    format: None,
                    reported_status: None,
                    reported_lot_count: None,
                    schedule: Default::default(),
                },
            })
        }),
        attributes,
    };
    let raw_values = serde_json::to_value(raw_values)
        .map_err(NormalizationInputError::JsonSerialization)
        .and_then(RawProductListingValues::new)?;
    let context = NormalizationContext::new(json!({
        "baseUrl": candidate_url,
        "fallbackCurrency": fallback_currency.map(|currency| currency.as_str()),
    }))?;

    ProductListingNormalizationInput::new(
        RawProductListingOperation::Upsert,
        RawProductListingPayloadFormat::CrawlerExtractedProduct,
        1,
        1,
        source_payload,
        raw_values,
        context,
    )
}

/// Builds durable evidence for a crawler-verified removal without retaining HTML or response data.
pub(crate) fn crawler_verified_removal_input(
    candidate_url: &Url,
) -> Result<ProductListingNormalizationInput, NormalizationInputError> {
    ProductListingNormalizationInput::new(
        RawProductListingOperation::Delete,
        RawProductListingPayloadFormat::CrawlerExtractedProduct,
        1,
        1,
        SourcePayload::new(json!({
            "candidateUrl": candidate_url,
            "removalEvidence": "VERIFIED",
        }))?,
        RawProductListingValues::new(json!({}))?,
        NormalizationContext::new(json!({
            "baseUrl": candidate_url,
            "fallbackCurrency": null,
        }))?,
    )
}

/// Builds non-input crawler provenance. Page/schema data is excluded from the input hash.
pub(crate) fn crawler_provenance(
    page_hash: Option<&str>,
    schema_fingerprint: Option<&str>,
) -> Result<RawProductListingProvenance, NormalizationInputError> {
    RawProductListingProvenance::new(json!({
        "pageHash": page_hash,
        "schemaFingerprint": schema_fingerprint,
    }))
}

fn price_patch(value: Option<String>, resolved: bool) -> ProductListingRawValuesPatch<String> {
    if resolved {
        patch(value)
    } else {
        ProductListingRawValuesPatch::Clear
    }
}

fn patch(value: Option<String>) -> ProductListingRawValuesPatch<String> {
    match value {
        Some(value) => ProductListingRawValuesPatch::Set(value),
        None => ProductListingRawValuesPatch::Clear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::IMAGE_CANDIDATE_SEPARATOR;

    fn raw() -> RawExtractedProduct {
        RawExtractedProduct {
            source_listing_id: "SKU-1".to_owned(),
            title: "Chair".to_owned(),
            description: vec!["Oak".to_owned()],
            price: Some("100 EUR".to_owned()),
            price_estimate_min: None,
            price_estimate_max: None,
            state: "In Stock".to_owned(),
            images: vec!["/chair.jpg".to_owned()],
            raw_attributes: BTreeMap::new(),
        }
    }

    #[test]
    fn should_preserve_selected_raw_values_in_capture_input() -> Result<(), NormalizationInputError>
    {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let extracted = raw();
        let input = crawler_raw_input(
            &extracted,
            &extracted.images,
            &url,
            None,
            Some(Currency::Eur),
            [true, false, false],
        )?;

        assert_eq!(
            Some(&serde_json::Value::String("100 EUR".to_owned())),
            input.source_payload().value().get("price")
        );
        assert_eq!(
            Some(&serde_json::json!({"action": "SET", "value": "100 EUR"})),
            input.raw_values().value().get("price")
        );
        assert_eq!(
            Some(&serde_json::json!({"action": "UNCHANGED"})),
            input.raw_values().value().get("auction")
        );
        assert_eq!(
            Some(&serde_json::json!(["/chair.jpg"])),
            input.source_payload().value().get("images")
        );
        Ok(())
    }

    #[test]
    fn should_map_fixture_backed_auction_evidence_to_current_raw_context()
    -> Result<(), NormalizationInputError> {
        let url = Url::parse(
            "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd",
        )
        .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let extracted = raw();
        let evidence = CrawlerAuctionEvidence {
            source_auction_id: "leipzig10033".to_owned(),
            catalogue_url: "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033".to_owned(),
            name: Some("Auktion 9".to_owned()),
            lot_number: Some("54".to_owned()),
        };

        let input = crawler_raw_input(
            &extracted,
            &extracted.images,
            &url,
            Some(&evidence),
            Some(Currency::Eur),
            [true, false, false],
        )?;

        assert_eq!(
            Some(&serde_json::json!({
                "action": "SET",
                "value": {
                    "sourceAuctionId": {"action": "SET", "value": "leipzig10033"},
                    "lotNumber": "54",
                    "cataloguePosition": null,
                    "timing": null,
                    "auctionMetadata": {
                        "name": "Auktion 9",
                        "description": null,
                        "catalogueUrl": "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033",
                        "format": null,
                        "reportedStatus": null,
                        "reportedLotCount": null,
                        "schedule": {
                            "biddingOpens": null,
                            "liveStarts": null,
                            "lotsBeginClosing": null,
                            "scheduledEnd": null
                        }
                    }
                }
            })),
            input.raw_values().value().get("auction")
        );
        Ok(())
    }

    #[test]
    fn should_clear_unresolved_price_from_generic_raw_values() -> Result<(), NormalizationInputError>
    {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let extracted = raw();

        let input = crawler_raw_input(
            &extracted,
            &extracted.images,
            &url,
            None,
            None,
            [false, false, false],
        )?;

        assert_eq!(
            Some(&serde_json::Value::String("100 EUR".to_owned())),
            input.source_payload().value().get("price")
        );
        assert_eq!(
            Some(&serde_json::json!({"action": "CLEAR"})),
            input.raw_values().value().get("price")
        );
        Ok(())
    }

    #[test]
    fn should_preserve_image_groups_in_source_payload_and_use_validated_image_projection()
    -> Result<(), NormalizationInputError> {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let mut extracted = raw();
        extracted.images = vec![
            format!("/images/primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/fallback-800x600.jpg"),
            format!(
                "/images/second.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/second-fallback-640x480.jpg"
            ),
            format!("/images/primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/fallback-800x600.jpg"),
        ];
        let validated_image_urls = vec![
            "https://example.com/images/fallback-800x600.jpg".to_owned(),
            "https://example.com/images/second-fallback-640x480.jpg".to_owned(),
        ];

        let input = crawler_raw_input(
            &extracted,
            &validated_image_urls,
            &url,
            None,
            Some(Currency::Eur),
            [true, false, false],
        )?;

        assert_eq!(
            input.source_payload().value().get("images"),
            Some(&serde_json::json!([
                format!(
                    "/images/primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/fallback-800x600.jpg"
                ),
                format!(
                    "/images/second.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/second-fallback-640x480.jpg"
                ),
                format!(
                    "/images/primary.jpg{IMAGE_CANDIDATE_SEPARATOR}/images/fallback-800x600.jpg"
                ),
            ]))
        );
        assert_eq!(
            input.raw_values().value().get("images"),
            Some(&serde_json::json!({
                "action": "SET",
                "value": [
                    "https://example.com/images/fallback-800x600.jpg",
                    "https://example.com/images/second-fallback-640x480.jpg",
                ],
            }))
        );
        Ok(())
    }

    #[test]
    fn should_hash_dynamic_raw_attribute_changes() -> Result<(), NormalizationInputError> {
        let url = Url::parse("https://example.com/products/1")
            .unwrap_or_else(|error| panic!("static test URL must parse: {error}"));
        let extracted = raw();
        let first = crawler_raw_input(
            &extracted,
            &extracted.images,
            &url,
            None,
            Some(Currency::Eur),
            [true, false, false],
        )?
        .hash()?;
        let mut changed = raw();
        changed
            .raw_attributes
            .insert("rawMaterial".to_owned(), vec!["Oak".to_owned()]);
        let second = crawler_raw_input(
            &changed,
            &changed.images,
            &url,
            None,
            Some(Currency::Eur),
            [true, false, false],
        )?
        .hash()?;
        assert_ne!(first, second);
        Ok(())
    }
}

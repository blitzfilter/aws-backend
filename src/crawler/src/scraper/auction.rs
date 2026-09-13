//! Fixture-backed Auction evidence extractors for known source URL namespaces.
//!
//! This module intentionally has no generic URL matching. A provider rule must prove
//! both its source-key path and any page selectors with a checked-in fixture.

use crate::scraper::css_selector::product_schema::RawExtractedProduct;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrawlerAuctionEvidence {
    pub(crate) source_auction_id: String,
    pub(crate) catalogue_url: String,
    pub(crate) name: Option<String>,
    pub(crate) lot_number: Option<String>,
}

/// Extracts Lot-tissimo catalogue evidence from its tested lot URL namespace.
///
/// The source key is the nonempty suffix of `catalogue-id-…`. The rule requires
/// the complete known path and never falls back to a name, generic URL hash, or
/// a loosely matched path segment.
pub(crate) fn extract_lot_tissimo_auction(
    candidate_url: &Url,
    raw: &RawExtractedProduct,
) -> Option<CrawlerAuctionEvidence> {
    let host = candidate_url.host_str()?;
    if !matches!(host, "lot-tissimo.com" | "www.lot-tissimo.com")
        || candidate_url.scheme() != "https"
        || candidate_url.query().is_some()
        || candidate_url.fragment().is_some()
    {
        return None;
    }

    let segments = candidate_url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let [locale, catalogue_collection, auctioneer, catalogue, lot] = segments.as_slice() else {
        return None;
    };
    if !is_lot_tissimo_locale(locale)
        || *catalogue_collection != "auction-catalogues"
        || auctioneer.is_empty()
    {
        return None;
    }
    let source_auction_id = catalogue.strip_prefix("catalogue-id-")?;
    if source_auction_id.is_empty()
        || lot
            .strip_prefix("lot-")
            .is_none_or(|source_lot_id| source_lot_id.is_empty())
    {
        return None;
    }

    let mut catalogue_url = candidate_url.clone();
    catalogue_url.set_path(&format!(
        "/{locale}/{catalogue_collection}/{auctioneer}/{catalogue}"
    ));
    catalogue_url.set_query(None);
    catalogue_url.set_fragment(None);
    let catalogue_url = catalogue_url.to_string();

    Some(CrawlerAuctionEvidence {
        source_auction_id: source_auction_id.to_owned(),
        name: raw_attribute(raw, "rawAuctionName"),
        catalogue_url,
        lot_number: raw_attribute(raw, "rawAuctionLotNumber"),
    })
}

fn is_lot_tissimo_locale(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[2] == b'-'
        && bytes[0].is_ascii_lowercase()
        && bytes[1].is_ascii_lowercase()
        && bytes[3].is_ascii_lowercase()
        && bytes[4].is_ascii_lowercase()
}

fn raw_attribute(raw: &RawExtractedProduct, key: &str) -> Option<String> {
    raw.raw_attributes
        .get(key)
        .and_then(|values| values.first())
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
    use scraper::Html;
    use serde_json::Value;

    const LOT_TISSIMO_HTML: &str =
        include_str!("../../tests/fixtures/html/lot-tissimo_listed.html");
    const FIXTURES: &str = include_str!("../../tests/fixtures/fixtures.json");
    const LOT_URL: &str = "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd";

    fn raw() -> RawExtractedProduct {
        let fixtures: Value = serde_json::from_str(FIXTURES)
            .unwrap_or_else(|error| panic!("crawler fixture JSON: {error}"));
        let fixture = fixtures
            .as_array()
            .and_then(|fixtures| {
                fixtures.iter().find(|fixture| {
                    fixture.get("html").and_then(Value::as_str)
                        == Some("tests/fixtures/html/lot-tissimo_listed.html")
                })
            })
            .unwrap_or_else(|| panic!("Lot-tissimo fixture must exist"));
        let schema: ProductCssSelectorSchema = serde_json::from_value(
            fixture
                .get("schema")
                .cloned()
                .unwrap_or_else(|| panic!("Lot-tissimo fixture schema must exist")),
        )
        .unwrap_or_else(|error| panic!("Lot-tissimo fixture schema: {error}"));
        schema
            .apply(&Html::parse_document(LOT_TISSIMO_HTML))
            .unwrap_or_else(|error| panic!("Lot-tissimo fixture schema must apply: {error}"))
    }

    #[test]
    fn should_extract_catalogue_identity_and_selector_bound_evidence_from_fixture_backed_lot_tissimo_url()
     {
        let url = Url::parse(LOT_URL).unwrap_or_else(|error| panic!("fixture URL: {error}"));

        let evidence = extract_lot_tissimo_auction(&url, &raw())
            .unwrap_or_else(|| panic!("fixture must match documented Lot-tissimo rule"));

        assert_eq!("leipzig10033", evidence.source_auction_id);
        assert_eq!(
            "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033",
            evidence.catalogue_url
        );
        assert_eq!(Some("Auktion 9".to_owned()), evidence.name);
        assert_eq!(Some("54".to_owned()), evidence.lot_number);
    }

    #[test]
    fn should_reject_unproven_hosts_wrappers_and_path_shapes() {
        let raw = raw();
        for url in [
            "https://example.test/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd",
            "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd",
            "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033",
            "https://www.lot-tissimo.com/de-de/auction-catalogues/kunstauktionshaus-leipzig/catalogue-id-leipzig10033/lot-a2850590-e73c-4cce-9386-b3fd00b49bfd?utm_source=fixture",
        ] {
            let url = Url::parse(url).unwrap_or_else(|error| panic!("fixture URL: {error}"));
            assert!(extract_lot_tissimo_auction(&url, &raw).is_none(), "{url}");
        }
    }
}

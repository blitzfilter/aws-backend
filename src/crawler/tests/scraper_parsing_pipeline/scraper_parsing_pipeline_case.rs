use crawler::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use money::Currency;
use money::{MonetaryAmount, Price};
use product_listing_core::listing_availability::ListingAvailability;
use serde::Deserialize;

use crate::expectation_types::{NormalizedExpectation, NormalizedExpectationJson, RawExpectation};

pub struct ScraperParsingPipelineFixture {
    pub schemas: Vec<ProductCssSelectorSchema>,
    pub schema_index: usize,
    pub raw: RawExpectation,
    pub normalized: NormalizedExpectation,
    pub html_path: String,
}

impl ScraperParsingPipelineFixture {
    pub fn load_html_source(&self) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(&self.html_path);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed reading fixture html '{}': {e}", path.display()))
    }
}

#[derive(Debug, Deserialize)]
struct FixtureJson {
    html: String,
    schema: Option<ProductCssSelectorSchema>,
    schemas_file: Option<String>,
    #[serde(default)]
    schema_index: usize,
    raw: RawExpectation,
    normalized: NormalizedExpectationJson,
}

/// Load all fixture cases from `tests/fixtures/fixtures.json`.
///
/// Adding a new ListingSource or a new case for an existing ListingSource requires only:
///   1. Drop the HTML file in `tests/fixtures/html/<listing_source>[_variant].html`.
///   2. Add the ListingSource cached schemas to `tests/fixtures/schemas/<listing_source>.json`.
///   3. Append an element to `tests/fixtures/fixtures.json` with
///      `schemas_file`, `schema_index`, `html`, `raw`, and `normalized` fields.
///
/// No Rust code changes are needed.
pub fn load_all_fixtures() -> Vec<ScraperParsingPipelineFixture> {
    let full =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fixtures.json");
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed reading '{}': {e}", full.display()));
    let items: Vec<FixtureJson> = serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("failed parsing '{}': {e}", full.display()));

    assert!(
        !items.is_empty(),
        "tests/fixtures/fixtures.json must not be empty"
    );

    items.into_iter().map(fixture_from_json).collect()
}

fn fixture_from_json(f: FixtureJson) -> ScraperParsingPipelineFixture {
    let schemas = match (f.schema, f.schemas_file) {
        (Some(schema), None) => vec![schema],
        (None, Some(path)) => {
            let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
            let src = std::fs::read_to_string(&full)
                .unwrap_or_else(|e| panic!("failed reading schemas '{}': {e}", full.display()));
            serde_json::from_str(&src)
                .unwrap_or_else(|e| panic!("failed parsing schemas '{}': {e}", full.display()))
        }
        (Some(_), Some(_)) => panic!("fixture must set only schema or schemas_file"),
        (None, None) => panic!("fixture must set schema or schemas_file"),
    };
    assert!(
        f.schema_index < schemas.len(),
        "schema_index {} is out of range for {} schemas",
        f.schema_index,
        schemas.len()
    );

    ScraperParsingPipelineFixture {
        schemas,
        schema_index: f.schema_index,
        raw: f.raw,
        normalized: normalized_from_json(f.normalized),
        html_path: f.html,
    }
}

fn normalized_from_json(data: NormalizedExpectationJson) -> NormalizedExpectation {
    NormalizedExpectation {
        source_listing_id: data.source_listing_id,
        title: data.title,
        description: data.description.map(|description| {
            product_listing_core::description::Description::from(description.as_str()).to_string()
        }),
        price: price_from_parts(data.price, data.price_currency.as_deref()),
        price_estimate_min: price_from_parts(
            data.price_estimate_min,
            data.price_estimate_min_currency.as_deref(),
        ),
        price_estimate_max: price_from_parts(
            data.price_estimate_max,
            data.price_estimate_max_currency.as_deref(),
        ),

        availability: parse_availability(data.availability.as_deref()),
        url: data.url,
        images: data.images,
        auction_start: parse_optional_rfc3339(data.auction_start.as_deref()),
        auction_end: parse_optional_rfc3339(data.auction_end.as_deref()),
    }
}

fn price_from_parts(minor: Option<u64>, currency: Option<&str>) -> Option<Price> {
    match (minor, currency) {
        (None, None) => None,
        (Some(amount), Some(curr)) => Some(Price::new(
            MonetaryAmount::from(amount),
            parse_currency(curr),
        )),
        _ => panic!(
            "invalid price in fixtures.json: price_minor and price_currency must both be set or both be null"
        ),
    }
}

fn parse_currency(code: &str) -> Currency {
    match code {
        "EUR" => Currency::Eur,
        "USD" => Currency::Usd,
        "GBP" => Currency::Gbp,
        "AUD" => Currency::Aud,
        "CAD" => Currency::Cad,
        "NZD" => Currency::Nzd,
        "CNY" => Currency::Cny,
        "BRL" => Currency::Brl,
        "PLN" => Currency::Pln,
        "TRY" => Currency::Try,
        "JPY" => Currency::Jpy,
        "CZK" => Currency::Czk,
        "RUB" => Currency::Rub,
        "AED" => Currency::Aed,
        "SAR" => Currency::Sar,
        "HKD" => Currency::Hkd,
        "SGD" => Currency::Sgd,
        "CHF" => Currency::Chf,
        other => panic!("unsupported currency '{other}' in fixtures.json"),
    }
}

fn parse_availability(value: Option<&str>) -> Option<ListingAvailability> {
    value.map(|code| {
        ListingAvailability::from_code(code).unwrap_or_else(|| {
            panic!("unsupported normalized availability '{code}' in fixtures.json")
        })
    })
}

fn parse_optional_rfc3339(value: Option<&str>) -> Option<time::OffsetDateTime> {
    value.map(|v| {
        time::OffsetDateTime::parse(v, &time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|e| panic!("invalid RFC3339 datetime '{v}' in fixtures.json: {e}"))
    })
}

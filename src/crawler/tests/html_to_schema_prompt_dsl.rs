use crawler::scraper::css_selector::product_schema_service::html_to_schema_prompt_dsl;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct FixtureJson {
    html: String,
    raw: RawExpectation,
}

#[derive(Debug, Deserialize)]
struct RawExpectation {
    source_listing_id: String,
    title: String,
    description: Vec<String>,
    price: Option<String>,
    price_estimate_min: Option<String>,
    price_estimate_max: Option<String>,
    state: String,
    images: Vec<String>,
}

#[test]
fn should_project_all_html_fixtures_to_compact_schema_prompt_dsl() {
    for fixture in load_fixtures() {
        let html = read_fixture_html(&fixture.html);
        let dsl = html_to_schema_prompt_dsl(&html);

        assert!(
            !dsl.trim().is_empty(),
            "{} should produce non-empty DSL",
            fixture.html
        );
        assert!(
            dsl.len() < html.len(),
            "{} DSL should be smaller than raw HTML: dsl={} html={}",
            fixture.html,
            dsl.len(),
            html.len()
        );
        assert_no_raw_noise_blocks(&fixture.html, &dsl);
        assert_raw_expectations_are_represented(&fixture.html, &dsl, &fixture.raw);
        assert_fixture_specific_expectations(&fixture.html, &dsl);
    }
}

fn load_fixtures() -> Vec<FixtureJson> {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fixtures.json");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed reading '{}': {err}", path.display()));
    serde_json::from_str(&src)
        .unwrap_or_else(|err| panic!("failed parsing '{}': {err}", path.display()))
}

fn read_fixture_html(path: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed reading '{}': {err}", path.display()))
}

fn assert_no_raw_noise_blocks(fixture_path: &str, dsl: &str) {
    for needle in [
        "script", "style", "svg", "canvas", "nav", "header", "footer", "aside",
    ] {
        assert!(
            !dsl.contains(&format!("<{needle}")) && !dsl.contains(&format!("tag: {needle}")),
            "{fixture_path} DSL should not contain noise block {needle}"
        );
    }
}

fn assert_raw_expectations_are_represented(fixture_path: &str, dsl: &str, raw: &RawExpectation) {
    assert_value_is_represented(
        fixture_path,
        "source_listing_id",
        &raw.source_listing_id,
        dsl,
    );
    assert_value_is_represented(fixture_path, "title", &raw.title, dsl);
    assert_value_is_represented(fixture_path, "state", &raw.state, dsl);
    assert_optional_value_is_represented(fixture_path, "price", raw.price.as_deref(), dsl);
    assert_optional_value_is_represented(
        fixture_path,
        "price_estimate_min",
        raw.price_estimate_min.as_deref(),
        dsl,
    );
    assert_optional_value_is_represented(
        fixture_path,
        "price_estimate_max",
        raw.price_estimate_max.as_deref(),
        dsl,
    );

    if let Some(description) = raw.description.first() {
        assert_value_is_represented(fixture_path, "description", description, dsl);
    }
    if let Some(image) = raw.images.first() {
        assert_value_is_represented(fixture_path, "image", image, dsl);
    }
}

fn assert_optional_value_is_represented(
    fixture_path: &str,
    field: &str,
    value: Option<&str>,
    dsl: &str,
) {
    if let Some(value) = value {
        assert_value_is_represented(fixture_path, field, value, dsl);
    }
}

fn assert_value_is_represented(fixture_path: &str, field: &str, value: &str, dsl: &str) {
    let value = normalize_probe_text(value);
    if value.is_empty() {
        return;
    }
    let probe = value.chars().take(180).collect::<String>();
    let normalized_dsl = normalize_probe_text(dsl);
    let compact_probe = compact_probe_text(&probe);
    let compact_dsl = compact_probe_text(&normalized_dsl);
    assert!(
        normalized_dsl.contains(&probe)
            || compact_dsl.contains(&compact_probe)
            || significant_words_are_represented(&probe, &normalized_dsl),
        "{fixture_path} DSL should contain expected raw {field} value {probe:?}"
    );
}

fn normalize_probe_text(value: &str) -> String {
    value
        .replace("\\u2026", "...")
        .replace('…', "...")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn compact_probe_text(value: &str) -> String {
    value.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn significant_words_are_represented(probe: &str, dsl: &str) -> bool {
    let words = probe
        .split(|ch: char| !ch.is_alphanumeric())
        .flat_map(split_concatenated_word)
        .filter(|word| word.chars().count() > 2)
        .take(8)
        .collect::<Vec<_>>();

    !words.is_empty() && words.iter().all(|word| dsl.contains(word))
}

fn split_concatenated_word(word: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in word.chars() {
        if ch.is_uppercase()
            && current
                .chars()
                .last()
                .is_some_and(|previous| previous.is_lowercase())
            && !current.is_empty()
        {
            parts.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

fn assert_fixture_specific_expectations(fixture_path: &str, dsl: &str) {
    match fixture_path {
        path if path.contains("lot-tissimo") => {
            for needle in [
                "id: LotId",
                "id: ClientName",
                "id: lot-is-ended",
                "property: og:title",
                "property: og:description",
                "Kunstauktionshaus Leipzig",
                "cdn.globalauctionplatform.com",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        path if path.contains("chairish") => {
            for needle in [
                "property: og:product-id",
                "property: product:availability",
                "class: product-title",
                "js-product-description",
                "tag: img",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        path if path.contains("weitze") => {
            for needle in [
                "tag: h1",
                "itemprop: name",
                "tag: span",
                "itemprop: price",
                "itemprop: image",
                "405500",
                "25,00",
                "405500.webp",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        path if path.contains("antik-und-stil_available") => {
            for needle in [
                "sku",
                "product_title",
                "single_add_to_cart_button",
                "tag: img",
                "2025-0219-03",
                "Tisch rund antik Marmorplatte",
                "In den Warenkorb",
                "Tisch-Marmor1000",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        path if path.contains("antik-und-stil_sale") => {
            for needle in [
                "product_title",
                "woocommerce-product-gallery__image",
                "elementor-widget-woocommerce-product-content",
                "38426",
                "Couchtisch Glas Vintage",
                "In den Warenkorb",
                "56A8EEE6-078D-496D-8362-FDF3E6170795.webp",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        path if path.contains("antik-und-stil_priceless") => {
            for needle in [
                "product_title",
                "woocommerce-product-gallery__image",
                "elementor-widget-woocommerce-product-content",
                "38291",
                "Antiker Jugendstil Kronleuchter",
                "In den Warenkorb",
                "Kein-Titel-105-x-105-mm",
            ] {
                assert!(dsl.contains(needle), "{fixture_path} DSL missing {needle}");
            }
        }
        _ => {}
    }
}

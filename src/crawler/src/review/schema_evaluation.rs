use crate::review::model::{
    CrawlerReviewPage, SchemaCandidateEvaluation, SchemaMatrix, SchemaPageEvaluation,
    SchemaReviewPageInput, SelectorFieldEvaluation,
};
use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::scraper_service::extraction::engine::apply_schema_to_document;
use scraper::{Html, Selector};

pub(crate) fn evaluate_schema_matrix_for_live_review_pages(
    review_id: uuid::Uuid,
    schemas: &[ProductCssSelectorSchema],
    pages: &[(CrawlerReviewPage, String)],
) -> SchemaMatrix {
    let mut candidates = Vec::with_capacity(schemas.len());
    for (schema_index, schema) in schemas.iter().enumerate() {
        let mut page_evaluations = Vec::with_capacity(pages.len());
        for (page, raw_html) in pages {
            page_evaluations.push(evaluate_schema_review_page(schema, page, raw_html));
        }
        candidates.push(SchemaCandidateEvaluation {
            schema_index,
            pages: page_evaluations,
        });
    }

    SchemaMatrix {
        review_id,
        candidates,
    }
}

pub(crate) fn evaluate_schema_matrix_for_inputs(
    schemas: &[ProductCssSelectorSchema],
    pages: &[SchemaReviewPageInput],
) -> SchemaMatrix {
    let mut candidates = Vec::with_capacity(schemas.len());
    for (schema_index, schema) in schemas.iter().enumerate() {
        let mut page_evaluations = Vec::with_capacity(pages.len());
        for (page_index, page) in pages.iter().enumerate() {
            page_evaluations.push(evaluate_schema_input_page(schema, page_index, page));
        }
        candidates.push(SchemaCandidateEvaluation {
            schema_index,
            pages: page_evaluations,
        });
    }

    SchemaMatrix {
        review_id: uuid::Uuid::nil(),
        candidates,
    }
}

pub(crate) fn schema_matrix_has_required_coverage(matrix: &SchemaMatrix) -> bool {
    let Some(first_candidate) = matrix.candidates.first() else {
        return false;
    };
    if first_candidate.pages.is_empty() {
        return false;
    }

    first_candidate.pages.iter().all(|target_page| {
        matrix.candidates.iter().any(|candidate| {
            candidate
                .pages
                .iter()
                .find(|page| page.page_id == target_page.page_id)
                .is_some_and(page_has_required_extraction)
        })
    })
}

pub(crate) fn unused_schema_indices(matrix: &SchemaMatrix) -> Vec<usize> {
    matrix
        .candidates
        .iter()
        .filter(|candidate| !candidate.pages.iter().any(page_has_required_extraction))
        .map(|candidate| candidate.schema_index)
        .collect()
}

fn evaluate_schema_review_page(
    schema: &ProductCssSelectorSchema,
    page: &CrawlerReviewPage,
    raw_html: &str,
) -> SchemaPageEvaluation {
    evaluate_schema_page(
        schema,
        page.review_page_id,
        page.url.clone(),
        page.role.clone(),
        raw_html,
    )
}

fn evaluate_schema_input_page(
    schema: &ProductCssSelectorSchema,
    page_index: usize,
    page: &SchemaReviewPageInput,
) -> SchemaPageEvaluation {
    evaluate_schema_page(
        schema,
        uuid::Uuid::from_u128(page_index as u128 + 1),
        page.url.clone(),
        page.role.clone(),
        &page.raw_html,
    )
}

fn evaluate_schema_page(
    schema: &ProductCssSelectorSchema,
    page_id: uuid::Uuid,
    url: String,
    role: String,
    raw_html: &str,
) -> SchemaPageEvaluation {
    let html = Html::parse_document(raw_html);
    let apply_result = apply_schema_to_document(schema, &html);
    let fields = evaluate_schema_fields(schema, raw_html);

    match apply_result {
        Ok(extracted) => SchemaPageEvaluation {
            page_id,
            url,
            role,
            apply_ok: true,
            extracted: Some(extracted),
            error: None,
            fields,
        },
        Err(err) => SchemaPageEvaluation {
            page_id,
            url,
            role,
            apply_ok: false,
            extracted: None,
            error: Some(err.to_string()),
            fields,
        },
    }
}

fn page_has_required_extraction(page: &SchemaPageEvaluation) -> bool {
    page.apply_ok
        && page
            .extracted
            .as_ref()
            .is_some_and(raw_extraction_has_required_values)
}

fn raw_extraction_has_required_values(raw: &RawExtractedProduct) -> bool {
    !raw.title.trim().is_empty() && !raw.state.trim().is_empty()
}

fn evaluate_schema_fields(
    schema: &ProductCssSelectorSchema,
    html: &str,
) -> Vec<SelectorFieldEvaluation> {
    let mut fields = Vec::new();
    if let Some(rule) = &schema.source_listing_id {
        fields.push(evaluate_rule("source_listing_id", rule, html));
    }
    fields.push(evaluate_rule("title", &schema.title, html));
    if let Some(rule) = &schema.description {
        fields.push(evaluate_rule("description", rule, html));
    }
    if let Some(rule) = &schema.price {
        fields.push(evaluate_rule("price", rule, html));
    }
    if let Some(rule) = &schema.price_estimate_min {
        fields.push(evaluate_rule("price_estimate_min", rule, html));
    }
    if let Some(rule) = &schema.price_estimate_max {
        fields.push(evaluate_rule("price_estimate_max", rule, html));
    }
    fields.push(evaluate_rule("state", &schema.state, html));
    fields.push(evaluate_rule("images", &schema.images, html));
    if let Some(rule) = &schema.auction_start {
        fields.push(evaluate_rule("auction_start", rule, html));
    }
    if let Some(rule) = &schema.auction_end {
        fields.push(evaluate_rule("auction_end", rule, html));
    }
    for (field, rule) in &schema.raw_attributes {
        fields.push(evaluate_rule(field, rule, html));
    }
    fields
}

fn evaluate_rule(
    field: &str,
    rule: &crate::scraper::css_selector::rule::ExtractionRule,
    html: &str,
) -> SelectorFieldEvaluation {
    let document = Html::parse_document(html);
    let selector = rule.selector.to_string();
    let selector_match_count = match Selector::parse(&selector) {
        Ok(parsed) => Some(document.select(&parsed).count()),
        Err(err) => {
            let error = format!("{err:?}");
            return SelectorFieldEvaluation {
                field: field.to_string(),
                selector: selector.clone(),
                selector_match_count: None,
                additional_selector_match_counts: Vec::new(),
                error: Some(error),
            };
        }
    };

    let mut additional_selector_match_counts = Vec::new();
    for additional in &rule.additional_selectors {
        match Selector::parse(additional.as_ref()) {
            Ok(parsed) => additional_selector_match_counts.push(document.select(&parsed).count()),
            Err(_) => additional_selector_match_counts.push(0),
        }
    }

    SelectorFieldEvaluation {
        field: field.to_string(),
        selector,
        selector_match_count,
        additional_selector_match_counts,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{PAGE_ROLE_PRIMARY, PAGE_ROLE_SEED};
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };

    fn text_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: Vec::new(),
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn image_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: Vec::new(),
            extract: ExtractionKind::Attribute { name: "src".into() },
            cardinality: ExtractionCardinality::All,
        }
    }

    fn schema(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            source_listing_id: Some(text_rule("#product-id")),
            title: text_rule(title_selector),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: image_rule("img"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
            raw_attributes: Default::default(),
        }
    }

    fn schema_with_price(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            price: Some(text_rule("#price")),
            ..schema(title_selector)
        }
    }

    fn schema_with_raw_attributes(title_selector: &str) -> ProductCssSelectorSchema {
        ProductCssSelectorSchema {
            raw_attributes: [
                ("rawShipment".to_string(), text_rule("#shipping")),
                ("rawCondition".to_string(), text_rule("#condition")),
                ("rawMaterial".to_string(), text_rule("#material")),
                ("rawYear".to_string(), text_rule("#year")),
                ("rawCategory".to_string(), text_rule("#category")),
                ("rawMeasurements".to_string(), text_rule("#measurements")),
                ("rawOrigin".to_string(), text_rule("#origin")),
            ]
            .into(),
            ..schema(title_selector)
        }
    }

    fn page(role: &str, title_tag: &str) -> SchemaReviewPageInput {
        SchemaReviewPageInput {
            url: format!("https://example.com/{role}"),
            role: role.to_string(),
            raw_html: format!(
                "<html><body><span id=\"product-id\">SKU</span><{title_tag}>Title</{title_tag}><span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>"
            ),
        }
    }

    fn page_with_price(role: &str, title_tag: &str, price: Option<&str>) -> SchemaReviewPageInput {
        let price_html = price
            .map(|value| format!("<span id=\"price\">{value}</span>"))
            .unwrap_or_default();

        SchemaReviewPageInput {
            url: format!("https://example.com/{role}"),
            role: role.to_string(),
            raw_html: format!(
                "<html><body><span id=\"product-id\">SKU</span><{title_tag}>Title</{title_tag}>{price_html}<span id=\"state\">Available</span><img src=\"a.jpg\"></body></html>"
            ),
        }
    }

    #[test]
    fn should_require_all_pages_to_be_covered_by_some_schema() {
        let schemas = vec![schema("h1"), schema("h2")];
        let pages = vec![page(PAGE_ROLE_PRIMARY, "h1"), page(PAGE_ROLE_SEED, "h2")];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(schema_matrix_has_required_coverage(&matrix));
    }

    #[test]
    fn should_reject_matrix_when_a_page_has_no_applicable_schema() {
        let schemas = vec![schema("h1")];
        let pages = vec![page(PAGE_ROLE_PRIMARY, "h1"), page(PAGE_ROLE_SEED, "h2")];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(!schema_matrix_has_required_coverage(&matrix));
    }

    #[test]
    fn should_reject_schema_when_non_null_optional_rule_is_absent() {
        let schemas = vec![schema_with_price("h1")];
        let pages = vec![page_with_price(PAGE_ROLE_PRIMARY, "h1", None)];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(!schema_matrix_has_required_coverage(&matrix));
        assert!(!matrix.candidates[0].pages[0].apply_ok);
    }

    #[test]
    fn should_cover_mixed_templates_with_split_schemas_for_price_presence() {
        let schemas = vec![schema_with_price("h1"), schema("h2")];
        let pages = vec![
            page_with_price(PAGE_ROLE_PRIMARY, "h1", Some("10 EUR")),
            page_with_price(PAGE_ROLE_SEED, "h2", None),
        ];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(schema_matrix_has_required_coverage(&matrix));
    }

    #[test]
    fn should_cover_product_page_when_images_are_absent() {
        let schemas = vec![ProductCssSelectorSchema {
            images: image_rule("#gallery img"),
            ..schema("h1")
        }];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/no-image-product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><span id=\"state\">Available</span><div id=\"gallery\"></div></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(schema_matrix_has_required_coverage(&matrix));
        assert!(
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .is_some_and(|raw| raw.images.is_empty())
        );
    }

    #[test]
    fn should_report_unused_schema_without_failing_page_coverage() {
        let schemas = vec![schema("h1"), schema("h3")];
        let pages = vec![page(PAGE_ROLE_PRIMARY, "h1")];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);

        assert!(schema_matrix_has_required_coverage(&matrix));
        assert_eq!(unused_schema_indices(&matrix), vec![1]);
    }

    #[test]
    fn should_preserve_image_candidate_groups_in_review_extraction() {
        let schema = ProductCssSelectorSchema {
            images: ExtractionRule {
                selector: CssSelector::from("img"),
                additional_selectors: Vec::new(),
                extract: ExtractionKind::ImageUrl,
                cardinality: ExtractionCardinality::All,
            },
            ..schema("h1")
        };
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><span id=\"state\">Available</span><img data-large_image=\"/images/primary.jpg\" src=\"/images/fallback-800x600.jpg\"><img data-large_image=\"/images/primary.jpg\" src=\"/images/fallback-800x600.jpg\"></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&[schema], &pages);

        assert_eq!(
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .map(|raw| raw.images.clone()),
            Some(vec![
                "/images/primary.jpg\u{1f}/images/fallback-800x600.jpg".to_string(),
                "/images/primary.jpg\u{1f}/images/fallback-800x600.jpg".to_string(),
            ])
        );
    }

    #[test]
    fn should_report_raw_attribute_selector_fields_when_configured() {
        let schemas = vec![schema_with_raw_attributes("h1")];
        let pages = vec![SchemaReviewPageInput {
            url: "https://example.com/product".to_string(),
            role: PAGE_ROLE_PRIMARY.to_string(),
            raw_html: "<html><body><span id=\"product-id\">SKU</span><h1>Title</h1><span id=\"state\">Available</span><img src=\"a.jpg\"><span id=\"shipping\">4 - 6 weeks</span><span id=\"condition\">Good</span><span id=\"material\">Walnut</span><span id=\"year\">1830</span><span id=\"category\">Furniture</span><span id=\"measurements\">H 90 cm</span><span id=\"origin\">Germany</span></body></html>".to_string(),
        }];

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let fields = &matrix.candidates[0].pages[0].fields;

        assert!(fields.iter().any(|field| field.field == "rawShipment"));
        assert!(fields.iter().any(|field| field.field == "rawCondition"));
        assert!(fields.iter().any(|field| field.field == "rawMaterial"));
        assert!(fields.iter().any(|field| field.field == "rawYear"));
        assert!(fields.iter().any(|field| field.field == "rawCategory"));
        assert!(fields.iter().any(|field| field.field == "rawMeasurements"));
        assert!(fields.iter().any(|field| field.field == "rawOrigin"));
        assert_eq!(
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .and_then(|raw| raw.raw_attributes.get("rawShipment")),
            Some(&vec!["4 - 6 weeks".to_string()])
        );
        assert_eq!(
            matrix.candidates[0].pages[0]
                .extracted
                .as_ref()
                .and_then(|raw| raw.raw_attributes.get("rawMaterial")),
            Some(&vec!["Walnut".to_string()])
        );
    }
}

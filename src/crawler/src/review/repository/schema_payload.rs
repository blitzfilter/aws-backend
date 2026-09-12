use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::rule::ExtractionRule;

use super::ReviewRepositoryError;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductSchemaReviewCandidate {
    schemas: Vec<ProductCssSelectorSchema>,
}

pub(super) fn parse_schemas_payload(
    candidate_payload: &serde_json::Value,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let candidate =
        serde_json::from_value::<ProductSchemaReviewCandidate>(candidate_payload.clone())
            .map_err(|_| ReviewRepositoryError::InvalidProductSchemaCandidate)?;
    validate_product_schemas(&candidate.schemas)?;
    Ok(candidate.schemas)
}

pub(super) fn validate_product_schemas(
    schemas: &[ProductCssSelectorSchema],
) -> Result<(), ReviewRepositoryError> {
    if schemas.is_empty() || schemas.iter().any(|schema| !valid_schema(schema)) {
        return Err(ReviewRepositoryError::InvalidProductSchemaCandidate);
    }
    Ok(())
}

fn valid_schema(schema: &ProductCssSelectorSchema) -> bool {
    let rules = [
        schema.source_listing_id.as_ref(),
        Some(&schema.title),
        schema.description.as_ref(),
        schema.price.as_ref(),
        schema.price_estimate_min.as_ref(),
        schema.price_estimate_max.as_ref(),
        Some(&schema.state),
        Some(&schema.images),
    ];

    rules
        .into_iter()
        .flatten()
        .chain(schema.raw_attributes.values())
        .all(|rule| rule.validate().is_ok())
}

pub(super) fn approval_product_schemas(
    reason: &str,
    existing: Option<&ListingSourceProductSchema>,
    reviewed_schemas: Vec<ProductCssSelectorSchema>,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let should_backfill_existing = matches!(
        reason,
        "append_schema_generation" | "normalization_schema_repair"
    ) && reviewed_schemas.len() == 1;

    let schemas = if !should_backfill_existing {
        reviewed_schemas
    } else if let Some(existing) = existing {
        merge_product_schema_lists(&existing.product_schemas, reviewed_schemas)?
    } else {
        reviewed_schemas
    };
    validate_product_schemas(&schemas)?;
    Ok(schemas)
}

pub(super) fn update_schema_field_payload(
    candidate_payload: &serde_json::Value,
    schema_index: usize,
    field: &str,
    rule: Option<ExtractionRule>,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let mut schemas = parse_schemas_payload(candidate_payload)?;
    let Some(schema) = schemas.get_mut(schema_index) else {
        return Err(ReviewRepositoryError::InvalidSchemaField(format!(
            "schema index {schema_index}"
        )));
    };

    update_schema_rule(schema, field, rule)?;
    validate_product_schemas(&schemas)?;
    Ok(schemas)
}

fn update_schema_rule(
    schema: &mut ProductCssSelectorSchema,
    field: &str,
    rule: Option<ExtractionRule>,
) -> Result<(), ReviewRepositoryError> {
    match field {
        "source_listing_id" => schema.source_listing_id = rule,
        "title" => {
            schema.title =
                rule.ok_or_else(|| ReviewRepositoryError::RequiredSchemaField(field.into()))?;
        }
        "state" => {
            schema.state =
                rule.ok_or_else(|| ReviewRepositoryError::RequiredSchemaField(field.into()))?;
        }
        "images" => {
            schema.images =
                rule.ok_or_else(|| ReviewRepositoryError::RequiredSchemaField(field.into()))?;
        }
        "description" => schema.description = rule,
        "price" => schema.price = rule,
        "price_estimate_min" => schema.price_estimate_min = rule,
        "price_estimate_max" => schema.price_estimate_max = rule,
        other => return Err(ReviewRepositoryError::InvalidSchemaField(other.into())),
    }
    Ok(())
}

fn merge_product_schema_lists(
    existing_schemas: &[ProductCssSelectorSchema],
    reviewed_schemas: Vec<ProductCssSelectorSchema>,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    let mut merged = existing_schemas.to_vec();
    let mut seen = existing_schemas
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()?;

    for schema in reviewed_schemas {
        let key = serde_json::to_value(&schema)?;
        if !seen.contains(&key) {
            seen.push(key);
            merged.push(schema);
        }
    }

    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };

    fn text_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        }
    }

    fn image_rule(selector: &str) -> ExtractionRule {
        ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
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
            raw_attributes: Default::default(),
        }
    }

    #[test]
    fn rejects_missing_empty_and_unknown_product_schema_candidate_fields() {
        for payload in [
            serde_json::json!({}),
            serde_json::json!({ "schemas": [] }),
            serde_json::json!({ "schemas": [], "unexpected": true }),
        ] {
            assert!(matches!(
                parse_schemas_payload(&payload),
                Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
            ));
        }
    }

    #[test]
    fn parses_non_empty_product_schema_candidate() {
        let payload = serde_json::json!({ "schemas": [schema("h1.product")] });

        let parsed = parse_schemas_payload(&payload);

        assert!(matches!(parsed, Ok(schemas) if schemas.len() == 1));
    }

    #[test]
    fn rejects_invalid_primary_and_additional_css_selectors() {
        let invalid_primary = serde_json::json!({ "schemas": [schema("[")] });
        let mut invalid_additional_schema = schema("h1.product");
        invalid_additional_schema
            .title
            .additional_selectors
            .push(CssSelector::from("["));
        let invalid_additional = serde_json::json!({ "schemas": [invalid_additional_schema] });

        for payload in [invalid_primary, invalid_additional] {
            assert!(matches!(
                parse_schemas_payload(&payload),
                Err(ReviewRepositoryError::InvalidProductSchemaCandidate)
            ));
        }
    }

    #[test]
    fn merge_product_schema_lists_appends_new_schema_after_existing_schemas() {
        let existing_a = schema("h1.template-a");
        let existing_b = schema("h1.template-b");
        let appended = schema("h1.template-c");

        let merged = merge_product_schema_lists(
            &[existing_a.clone(), existing_b.clone()],
            vec![appended.clone()],
        )
        .expect("schema merge should serialize");

        assert_eq!(merged, vec![existing_a, existing_b, appended]);
    }

    #[test]
    fn merge_product_schema_lists_deduplicates_existing_schema() {
        let existing = schema("h1.template-a");

        let merged =
            merge_product_schema_lists(std::slice::from_ref(&existing), vec![existing.clone()])
                .expect("schema merge should serialize");

        assert_eq!(merged, vec![existing]);
    }

    #[test]
    fn update_schema_rule_deletes_optional_rule() {
        let mut schema = schema("h1.template-a");
        schema.price = Some(text_rule(".price"));

        let schemas = update_schema_field_payload(
            &serde_json::json!({ "schemas": [schema] }),
            0,
            "price",
            None,
        )
        .expect("optional price rule can be deleted");

        assert!(schemas[0].price.is_none());
    }

    #[test]
    fn update_schema_rule_rejects_required_rule_deletion() {
        let schema = schema("h1.template-a");

        let err = update_schema_field_payload(
            &serde_json::json!({ "schemas": [schema] }),
            0,
            "title",
            None,
        )
        .unwrap_err();

        assert!(
            matches!(err, ReviewRepositoryError::RequiredSchemaField(field) if field == "title")
        );
    }

    #[test]
    fn update_schema_field_payload_rejects_invalid_new_rule() {
        let err = update_schema_field_payload(
            &serde_json::json!({ "schemas": [schema("h1.template-a")] }),
            0,
            "title",
            Some(text_rule("[")),
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ReviewRepositoryError::InvalidProductSchemaCandidate
        ));
    }

    #[test]
    fn update_schema_field_payload_rejects_empty_schema_payload() {
        let err =
            update_schema_field_payload(&serde_json::json!({ "schemas": [] }), 0, "price", None)
                .unwrap_err();

        assert!(matches!(
            err,
            ReviewRepositoryError::InvalidProductSchemaCandidate
        ));
    }
}

use crate::CrawlerReviewId;
use crate::review::model::SchemaMatrix;
use crate::review::model::{STATUS_APPROVED, SchemaReviewPageInput};
use crate::review::repository::SchemaReviewWithStatusInput;
use crate::review::schema_evaluation::{
    evaluate_schema_matrix_for_inputs, schema_matrix_has_required_coverage, unused_schema_indices,
};
use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::product_schema_service::{
    ProductListingSchemaServiceError, SchemaLlmEvaluation,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use serde_json::{Value, json};
use tracing::info;

pub(crate) enum GeneratedSchemaReviewOutcome {
    Persisted(ListingSourceProductSchema),
    PendingReview(CrawlerReviewId),
}

impl ScraperServiceImpl {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_generated_schema_review(
        &self,
        listing_source_id: &ListingSourceId,
        reason: &str,
        schemas: Vec<ProductCssSelectorSchema>,
        evaluation: SchemaLlmEvaluation,
        pages: Vec<SchemaReviewPageInput>,
        validation_summary: Value,
    ) -> Result<GeneratedSchemaReviewOutcome, ScraperError> {
        let Some(review_repository) = &self.review_repository else {
            let saved = self
                .schema_service
                .save_product_schemas(listing_source_id, schemas)
                .await?;
            return Ok(GeneratedSchemaReviewOutcome::Persisted(saved));
        };
        if !self.review_required {
            let saved = self
                .schema_service
                .save_product_schemas(listing_source_id, schemas)
                .await?;
            return Ok(GeneratedSchemaReviewOutcome::Persisted(saved));
        }

        let matrix = evaluate_schema_matrix_for_inputs(&schemas, &pages);
        let deterministic_approval_ok = schema_matrix_has_required_coverage(&matrix);
        let mut validation_summary =
            with_schema_matrix_summary(validation_summary, &matrix, deterministic_approval_ok);
        let approved_by_llm = should_auto_approve_generated_schema(
            self.schema_llm_review_mode,
            deterministic_approval_ok,
            &evaluation,
        );
        let evaluation = evaluation.with_approved_by_llm(approved_by_llm);
        validation_summary = with_auto_schema_evaluation(validation_summary, &evaluation);

        info!(
            mode = self.schema_llm_review_mode.as_str(),
            decision = ?evaluation.decision,
            confidence = ?evaluation.confidence,
            approved_by_llm,
            deterministic_approval_ok,
            "Schema generation confidence evaluated"
        );

        if approved_by_llm {
            let saved = self
                .schema_service
                .save_product_schemas(listing_source_id, schemas.clone())
                .await?;
            review_repository
                .create_schema_review_with_status(SchemaReviewWithStatusInput {
                    listing_source_id,
                    reason,
                    schemas: &schemas,
                    pages,
                    validation_summary,
                    status: STATUS_APPROVED,
                    notes: Some("Auto-approved by LLM schema generation confidence"),
                })
                .await
                .map_err(review_error_to_schema_service_error)?;
            return Ok(GeneratedSchemaReviewOutcome::Persisted(saved));
        }

        let review_id = review_repository
            .create_schema_review(
                listing_source_id,
                reason,
                &schemas,
                pages,
                validation_summary,
            )
            .await
            .map_err(review_error_to_schema_service_error)?;
        Ok(GeneratedSchemaReviewOutcome::PendingReview(review_id))
    }
}

fn with_schema_matrix_summary(
    mut validation_summary: Value,
    matrix: &SchemaMatrix,
    deterministic_approval_ok: bool,
) -> Value {
    let mut failed_fields = Vec::new();
    for candidate in &matrix.candidates {
        for page in &candidate.pages {
            for field in &page.fields {
                if field.field == "images" && field.selector_match_count == Some(0) {
                    continue;
                }
                let selector_missing = field.selector_match_count == Some(0);
                if selector_missing || field.error.is_some() {
                    failed_fields.push(json!({
                        "schema_index": candidate.schema_index,
                        "page_reference": page.page_reference,
                        "url": page.url,
                        "role": page.role,
                        "field": field.field,
                        "selector": field.selector,
                        "selector_match_count": field.selector_match_count,
                        "present_but_missing_rule": selector_missing,
                        "error": field.error,
                    }));
                }
            }
        }
    }

    let matrix_summary = json!({
        "deterministic_approval_ok": deterministic_approval_ok,
        "schema_candidate_count": matrix.candidates.len(),
        "unused_schema_indices": unused_schema_indices(matrix),
        "present_but_missing_rule_failures": failed_fields,
    });

    if let Some(object) = validation_summary.as_object_mut() {
        object.insert("schema_matrix".to_string(), json!(matrix));
        object.insert("schema_matrix_summary".to_string(), matrix_summary);
        validation_summary
    } else {
        json!({
            "summary": validation_summary,
            "schema_matrix": matrix,
            "schema_matrix_summary": matrix_summary,
        })
    }
}

fn with_auto_schema_evaluation(
    mut validation_summary: Value,
    evaluation: &SchemaLlmEvaluation,
) -> Value {
    let evaluation = serde_json::to_value(evaluation)
        .unwrap_or_else(|err| json!({ "serialization_error": err.to_string() }));
    if let Some(object) = validation_summary.as_object_mut() {
        object.insert("auto_schema_evaluation".to_string(), evaluation);
        validation_summary
    } else {
        json!({
            "summary": validation_summary,
            "auto_schema_evaluation": evaluation,
        })
    }
}

fn should_auto_approve_generated_schema(
    mode: crate::scraper::scraper_service::service::SchemaLlmReviewMode,
    deterministic_approval_ok: bool,
    evaluation: &SchemaLlmEvaluation,
) -> bool {
    mode.allows_auto_approval()
        && deterministic_approval_ok
        && evaluation.is_high_confidence_approval()
}

fn review_error_to_schema_service_error(
    err: crate::review::repository::ReviewRepositoryError,
) -> ProductListingSchemaServiceError {
    ProductListingSchemaServiceError::DatabaseError(sqlx::Error::Protocol(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{
        SchemaCandidateEvaluation, SchemaPageEvaluation, SchemaPageReference,
        SelectorFieldEvaluation,
    };
    use serde_json::json;

    #[test]
    fn should_record_present_but_missing_rule_failures_in_validation_summary() {
        let matrix = SchemaMatrix {
            review_id: None,
            candidates: vec![SchemaCandidateEvaluation {
                schema_index: 0,
                pages: vec![SchemaPageEvaluation {
                    page_reference: SchemaPageReference::Input { page_index: 0 },
                    url: "https://example.com/product".to_string(),
                    role: "PRIMARY".to_string(),
                    apply_ok: false,
                    extracted: None,
                    error: Some("failed to extract `price`".to_string()),
                    fields: vec![SelectorFieldEvaluation {
                        field: "price".to_string(),
                        selector: "#price".to_string(),
                        selector_match_count: Some(0),
                        additional_selector_match_counts: Vec::new(),
                        error: None,
                    }],
                }],
            }],
        };

        let summary = with_schema_matrix_summary(json!({ "schema_count": 1 }), &matrix, false);

        assert_eq!(
            summary["schema_matrix_summary"]["deterministic_approval_ok"],
            false
        );
        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"][0]["field"],
            "price"
        );
        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"][0]["present_but_missing_rule"],
            true
        );
    }

    #[test]
    fn should_not_record_missing_images_as_present_but_missing_rule_failure() {
        let matrix = SchemaMatrix {
            review_id: None,
            candidates: vec![SchemaCandidateEvaluation {
                schema_index: 0,
                pages: vec![SchemaPageEvaluation {
                    page_reference: SchemaPageReference::Input { page_index: 0 },
                    url: "https://example.com/product".to_string(),
                    role: "PRIMARY".to_string(),
                    apply_ok: true,
                    extracted: None,
                    error: None,
                    fields: vec![SelectorFieldEvaluation {
                        field: "images".to_string(),
                        selector: "#gallery img".to_string(),
                        selector_match_count: Some(0),
                        additional_selector_match_counts: Vec::new(),
                        error: None,
                    }],
                }],
            }],
        };

        let summary = with_schema_matrix_summary(json!({ "schema_count": 1 }), &matrix, true);

        assert_eq!(
            summary["schema_matrix_summary"]["present_but_missing_rule_failures"],
            json!([])
        );
    }

    #[test]
    fn should_auto_approve_only_high_confidence_generation_when_mode_allows_it() {
        let high = SchemaLlmEvaluation {
            decision: crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationDecision::Approve,
            confidence: crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationConfidence::High,
            approved_by_llm: false,
            summary: "good".to_string(),
            risks: Vec::new(),
        };
        let medium = SchemaLlmEvaluation {
            decision: crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationDecision::NeedsHumanReview,
            confidence: crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationConfidence::Medium,
            approved_by_llm: false,
            summary: "uncertain".to_string(),
            risks: Vec::new(),
        };

        assert!(should_auto_approve_generated_schema(
            crate::scraper::scraper_service::service::SchemaLlmReviewMode::AutoApproveHighConfidence,
            true,
            &high,
        ));
        assert!(!should_auto_approve_generated_schema(
            crate::scraper::scraper_service::service::SchemaLlmReviewMode::AutoApproveHighConfidence,
            true,
            &medium,
        ));
        assert!(!should_auto_approve_generated_schema(
            crate::scraper::scraper_service::service::SchemaLlmReviewMode::AutoApproveHighConfidence,
            false,
            &high,
        ));
        assert!(!should_auto_approve_generated_schema(
            crate::scraper::scraper_service::service::SchemaLlmReviewMode::ReportOnly,
            true,
            &high,
        ));
    }
}

use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::css_selector::product_schema_service::{
    GeneratedSingleSchema, SchemaLlmEvaluation,
};
use crate::scraper::css_selector::removed_page_schema::ListingSourceRemovedPageSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::engine::try_apply_schemas;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use time::OffsetDateTime;
use tracing::warn;
use url::Url;

impl ScraperServiceImpl {
    /// ProductListing schemas use the normal review gate. Removed and not-product
    /// classifications are destructive URL mutations, so only HIGH-confidence
    /// structured responses may reach their mutation paths.
    /// Generates a fresh schema for the current page and applies it, **without
    /// persisting**.
    ///
    /// Persistence happens in
    /// [`ScraperServiceImpl::generate_fresh_schema_for_page`] only after the
    /// generated schema also normalizes successfully. This keeps a generated
    /// schema that applies but produces garbage out of the ListingSource cache.
    ///
    /// On success returns the generated schema, its raw extraction, and the
    /// LLM evaluation so the caller can persist them together.
    #[tracing::instrument(
        skip(self, html, expected_last_captured_raw_input_sha256),
        fields(
            listing_source_id = %listing_source_id,
            url = %url,
        )
    )]
    pub(crate) async fn generate_single_schema_for_page(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        html: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<
        (
            ProductCssSelectorSchema,
            RawExtractedProduct,
            SchemaLlmEvaluation,
        ),
        ScraperError,
    > {
        if let Some(review_id) = self
            .pending_product_schema_review_id(listing_source_id)
            .await?
        {
            return Err(ScraperError::PendingSchemaReview {
                url: url.clone(),
                review_id,
            });
        }

        self.consume_llm_budget_or_err(listing_source_id, url)
            .await?;

        let generated = self
            .schema_service
            .generate_single_schema_for_page(html)
            .await?;
        let (generated_schema, evaluation) = match generated {
            GeneratedSingleSchema::ProductListing { schema, evaluation } => (*schema, evaluation),
            GeneratedSingleSchema::Removed { schema, evaluation } => {
                if evaluation.confidence != crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationConfidence::High {
                    return Err(ScraperError::SchemaClassificationRejected {
                        url: url.clone(),
                        details: "removed classification requires HIGH confidence".to_string(),
                    });
                }
                if !schema.matches(html) {
                    return Err(
                        crate::scraper::scraper_service::pipeline::fresh_schema_apply::page_classification_did_not_match(
                            url,
                            &schema.selector,
                        ),
                    );
                }
                self.save_removed_page_schema(listing_source_id, schema)
                    .await?;
                return Err(ScraperError::ProductListingRemoved {
                    url: url.clone(),
                    details: "fresh schema generation classified page as removed".to_string(),
                });
            }
            GeneratedSingleSchema::NotProduct { reason, evaluation } => {
                if evaluation.confidence != crate::scraper::css_selector::product_schema_service::SchemaLlmEvaluationConfidence::High {
                    return Err(ScraperError::SchemaClassificationRejected {
                        url: url.clone(),
                        details: "not-product classification requires HIGH confidence".to_string(),
                    });
                }
                self.mark_url_other_best_effort(
                    listing_source_id,
                    url,
                    expected_last_captured_raw_input_sha256,
                )
                .await;
                return Err(ScraperError::NotProductPage {
                    url: url.clone(),
                    details: reason,
                });
            }
        };

        let (_, raw) = match try_apply_schemas(std::iter::once(&generated_schema), html) {
            Ok(applied) => applied,
            Err(err) => {
                warn!(error = ?err, "Generated schema did not apply; discarding");
                return Err(ScraperError::SchemaRegenerationExhausted {
                    url: url.clone(),
                    attempts: 1,
                    last_error: Box::new(err),
                });
            }
        };

        Ok((generated_schema, raw, evaluation))
    }

    pub(crate) async fn save_removed_page_schema(
        &self,
        listing_source_id: &ListingSourceId,
        schema: RemovedPageSchema,
    ) -> Result<(), ScraperError> {
        let existing = self
            .removed_page_schema_repository
            .find_removed_page_schema(listing_source_id)
            .await
            .map_err(ScraperError::RemovedPageSchemaDatabaseError)?;

        match existing {
            Some(existing) => {
                let mut schemas = existing.removed_page_schemas;
                if !schemas.contains(&schema) {
                    schemas.push(schema);
                }
                self.removed_page_schema_repository
                    .update_removed_page_schema(listing_source_id, &schemas)
                    .await
                    .map_err(ScraperError::RemovedPageSchemaDatabaseError)?;
            }
            None => {
                let now = OffsetDateTime::now_utc();
                let row = ListingSourceRemovedPageSchema {
                    listing_source_id: *listing_source_id,
                    removed_page_schemas: vec![schema],
                    created: now,
                    updated: now,
                };
                self.removed_page_schema_repository
                    .insert_removed_page_schema(listing_source_id, &row)
                    .await
                    .map_err(ScraperError::RemovedPageSchemaDatabaseError)?;
            }
        }

        Ok(())
    }
}

pub(crate) fn page_classification_did_not_match(
    url: &Url,
    selector: &crate::scraper::css_selector::rule::CssSelector,
) -> ScraperError {
    ScraperError::SchemaRegenerationExhausted {
        url: url.clone(),
        attempts: 1,
        last_error: Box::new(
            crate::scraper::css_selector::product_schema::ApplySchemaError::Title(
                ExtractionError::NoElementMatched {
                    selector: selector.to_string(),
                },
            ),
        ),
    }
}

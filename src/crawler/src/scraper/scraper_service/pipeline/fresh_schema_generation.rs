use crate::review::model::{PAGE_ROLE_TRIGGERING_GENERATION_PAGE, SchemaReviewPageInput};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::normalization::error::{NormalizationError, NormalizationFailureScope};

use crate::scraper::normalization::product_normalization_service::{
    NormalizationFailure, NormalizationSuccess,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_review_gate::GeneratedSchemaReviewOutcome;
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::pipeline::cached_schema_selection::PreparedSchemaSelection;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use serde_json::json;
use tracing::debug;
use url::Url;

pub(crate) struct FreshSchemaGenerationContext<'a> {
    pub(crate) listing_source_id: &'a ListingSourceId,
    pub(crate) domain: &'a str,
    pub(crate) url: &'a Url,
    pub(crate) html: &'a str,
    pub(crate) existing_schemas: &'a [ProductCssSelectorSchema],
    pub(crate) expected_last_captured_raw_input_sha256: Option<&'a [u8]>,
}

impl ScraperServiceImpl {
    #[tracing::instrument(
        skip(self, ctx),
        fields(
            listing_source_id = %ctx.listing_source_id,
            domain = ctx.domain,
            url = %ctx.url,
            schema_count = ctx.existing_schemas.len()
        )
    )]
    pub(crate) async fn generate_fresh_schema_for_page(
        &self,
        ctx: FreshSchemaGenerationContext<'_>,
    ) -> Result<PreparedSchemaSelection, ScraperError> {
        let (generated_schema, reapplied, evaluation) = self
            .generate_single_schema_for_page(
                ctx.listing_source_id,
                ctx.url,
                ctx.html,
                ctx.expected_last_captured_raw_input_sha256,
            )
            .await?;

        let mut validated_raw = reapplied.clone();
        let images = std::mem::take(&mut validated_raw.images);
        validated_raw.images =
            match filter_valid_image_urls(images, ctx.url, &*self.image_validator).await {
                Ok(images) => images,
                Err(NormalizationError::NoValidImages { .. }) => Vec::new(),
                Err(norm_err)
                    if norm_err.failure_scope() == NormalizationFailureScope::CandidateData =>
                {
                    return Err(ScraperError::FreshSchemaNormalizationFailed {
                        url: ctx.url.clone(),
                        attempts: 1,
                        last_norm_error: Box::new(norm_err),
                    });
                }
                Err(norm_err) => return Err(ScraperError::NormalizationError(norm_err)),
            };
        let validated_image_urls = validated_raw.images.clone();

        match self
            .normalization_service
            .normalize(
                validated_raw,
                ctx.url.clone(),
                generated_schema.default_currency.map(money::Currency::from),
            )
            .await
        {
            Ok(NormalizationSuccess {
                prepared,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(ctx.listing_source_id, ctx.url, llm_calls_used)
                    .await?;
                let mut persisted_schemas = ctx.existing_schemas.to_vec();
                persisted_schemas.push(generated_schema.clone());
                let pages = vec![SchemaReviewPageInput {
                    url: ctx.url.to_string(),
                    role: PAGE_ROLE_TRIGGERING_GENERATION_PAGE.to_string(),
                    raw_html: ctx.html.to_string(),
                }];
                match self
                    .handle_generated_schema_review(
                        ctx.listing_source_id,
                        "fresh_schema_generation",
                        persisted_schemas,
                        evaluation,
                        pages,
                        json!({ "schema_applied": true, "fresh_generation": true }),
                    )
                    .await?
                {
                    GeneratedSchemaReviewOutcome::Persisted(_) => {
                        debug!(domain = ctx.domain, url = %ctx.url, "Freshly generated schema produced valid product");
                        Ok(PreparedSchemaSelection {
                            prepared,
                            raw: reapplied,
                            validated_image_urls,
                            default_currency: generated_schema
                                .default_currency
                                .map(money::Currency::from),
                            schema: generated_schema,
                            fresh_schema: true,
                        })
                    }
                    GeneratedSchemaReviewOutcome::PendingReview(review_id) => {
                        Err(ScraperError::PendingSchemaReview {
                            url: ctx.url.clone(),
                            review_id,
                        })
                    }
                }
            }
            Err(NormalizationFailure {
                error: norm_err,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(ctx.listing_source_id, ctx.url, llm_calls_used)
                    .await?;
                if norm_err.failure_scope() == NormalizationFailureScope::CandidateData {
                    return Err(ScraperError::FreshSchemaNormalizationFailed {
                        url: ctx.url.clone(),
                        attempts: 1,
                        last_norm_error: Box::new(norm_err),
                    });
                }
                Err(ScraperError::NormalizationError(norm_err))
            }
        }
    }
}

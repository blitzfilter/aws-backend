use crate::scraper::css_selector::product_schema::{ProductCssSelectorSchema, RawExtractedProduct};
use crate::scraper::normalization::error::{NormalizationError, NormalizationFailureScope};

use crate::scraper::normalization::product_normalization_service::{
    NormalizationFailure, NormalizationSuccess, prepare_product,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_candidates::{
    PreparedSchemaCandidate, collect_applicable_candidates, rank_prepared_candidates,
    score_prepared_product,
};
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use money::Currency;
use tracing::debug;
use url::Url;

// ---------------------------------------------------------------------------
// ExistingSchemaSelection
// ---------------------------------------------------------------------------

/// Why fresh schema generation was requested after cached selection failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshSchemaGenerationReason {
    /// No cached schema could be applied to the current page.
    NoCachedSchemaApplied,
    /// One or more cached schemas applied, but none produced a valid
    /// prepared candidate.
    NoCachedSchemaNormalized,
}

impl FreshSchemaGenerationReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoCachedSchemaApplied => "no_cached_schema_applied",
            Self::NoCachedSchemaNormalized => "no_cached_schema_normalized",
        }
    }
}

pub(crate) struct PreparedSchemaSelection {
    pub(crate) prepared:
        crate::scraper::normalization::product_normalization_service::PreparedProduct,
    pub(crate) raw: RawExtractedProduct,
    pub(crate) default_currency: Option<Currency>,
    pub(crate) schema: ProductCssSelectorSchema,
    pub(crate) fresh_schema: bool,
}

pub(crate) enum ExistingSchemaSelection {
    Prepared(Box<PreparedSchemaSelection>),
    /// Cached selection cannot produce a valid product — generate a
    /// completely new schema for the current page. Never carries a cached
    /// schema as generation input.
    GenerateNewSchema {
        reason: FreshSchemaGenerationReason,
    },
}

// ---------------------------------------------------------------------------
// impl ScraperServiceImpl
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
    /// Applies every cached schema to the current page, ranks successful
    /// extractions by attribute completeness, and normalizes candidates from
    /// richest to least rich.
    ///
    /// The first candidate that normalizes successfully wins. When no cached
    /// candidate succeeds — either because none applied or none prepared —
    /// returns [`ExistingSchemaSelection::GenerateNewSchema`] so the caller
    /// falls back to fresh schema generation. No cached schema is ever
    /// selected as generation input.
    #[tracing::instrument(
        skip(self, schemas, html),
        fields(
            listing_source_id = %listing_source_id,
            url = %url,
            schema_count = schemas.len()
        )
    )]
    pub(crate) async fn select_existing_schema_with_normalization(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        html: &str,
        schemas: &[ProductCssSelectorSchema],
    ) -> Result<ExistingSchemaSelection, ScraperError> {
        // `scraper::Html` is `!Send`: parse and apply every cached schema in a
        // synchronous block so the parsed document is dropped before awaits.
        let candidates = {
            let parsed = scraper::Html::parse_document(html);
            let candidate_set = collect_applicable_candidates(schemas, &parsed);

            debug!(
                schema_candidates_total = schemas.len(),
                schema_candidates_applied = candidate_set.candidates.len(),
                schema_candidates_apply_failed = candidate_set.apply_failures.len(),
                "Cached schema application complete"
            );
            for (schema_index, err) in &candidate_set.apply_failures {
                debug!(
                    candidate_schema_index = schema_index,
                    candidate_apply_error = ?err,
                    "Cached schema failed to apply"
                );
            }

            candidate_set.candidates
        };

        let applied_schema_count = candidates.len();
        let mut prepared_candidates = Vec::new();
        // Image validation is candidate-local and does not call the LLM. Do
        // it before scoring so thumbnails and malformed URLs cannot inflate
        // richness.
        for mut candidate in candidates {
            candidate.raw.images =
                match filter_valid_image_urls(candidate.raw.images, url, &*self.image_validator)
                    .await
                {
                    Ok(images) => images,
                    Err(NormalizationError::NoValidImages { .. }) => Vec::new(),
                    Err(err) => return Err(ScraperError::NormalizationError(err)),
                };
            match prepare_product(
                candidate.raw.clone(),
                url.clone(),
                candidate.schema.default_currency.map(Into::into),
            ) {
                Ok(prepared) => {
                    let score = score_prepared_product(&candidate.raw, &prepared);
                    prepared_candidates.push(PreparedSchemaCandidate {
                        schema_index: candidate.schema_index,
                        schema: candidate.schema,
                        raw: candidate.raw,
                        prepared,
                        score,
                    });
                }
                Err(err) => {
                    debug!(
                        candidate_schema_index = candidate.schema_index,
                        candidate_rejection_reason = err.failure_reason(),
                        "Cached candidate rejected during deterministic preparation"
                    );
                }
            }
        }
        rank_prepared_candidates(&mut prepared_candidates);
        for candidate in &prepared_candidates {
            debug!(
                candidate_schema_index = candidate.schema_index,
                candidate_schema_score = candidate.score.as_usize(),
                "Ranked cached schema candidate"
            );
        }

        if prepared_candidates.is_empty() {
            let reason = if applied_schema_count == 0 {
                FreshSchemaGenerationReason::NoCachedSchemaApplied
            } else {
                FreshSchemaGenerationReason::NoCachedSchemaNormalized
            };
            debug!(
                fresh_schema_generation_reason = reason.as_str(),
                "No usable cached schema candidate; fresh schema generation required"
            );
            return Ok(ExistingSchemaSelection::GenerateNewSchema { reason });
        }

        for candidate in prepared_candidates {
            let raw = candidate.raw;
            match self
                .normalize_applied_schema(listing_source_id, url, candidate.schema, raw)
                .await
            {
                Ok(product) => {
                    debug!(
                        selected_schema_index = candidate.schema_index,
                        selected_schema_score = candidate.score.as_usize(),
                        candidate_normalization_result = "success",
                        "Cached schema selected"
                    );
                    return Ok(ExistingSchemaSelection::Prepared(Box::new(product)));
                }
                Err(ScraperError::NormalizationError(err))
                    if err.failure_scope() == NormalizationFailureScope::CandidateData =>
                {
                    debug!(
                        candidate_schema_index = candidate.schema_index,
                        candidate_schema_score = candidate.score.as_usize(),
                        candidate_rejection_reason = err.failure_reason(),
                        candidate_rejection_scope = "candidate_data",
                        candidate_normalization_result = "candidate_data_failure",
                        "Cached candidate normalization failed; trying next candidate"
                    );
                }
                Err(ScraperError::NormalizationError(err)) => {
                    debug!(
                        candidate_schema_index = candidate.schema_index,
                        candidate_schema_score = candidate.score.as_usize(),
                        normalization_failure_reason = err.failure_reason(),
                        normalization_failure_scope = "external",
                        "Cached candidate normalization failed externally; aborting selection"
                    );
                    return Err(ScraperError::NormalizationError(err));
                }
                Err(err) => return Err(err),
            }
        }

        debug!(
            fresh_schema_generation_reason =
                FreshSchemaGenerationReason::NoCachedSchemaNormalized.as_str(),
            "No cached schema normalized; fresh schema generation required"
        );
        Ok(ExistingSchemaSelection::GenerateNewSchema {
            reason: FreshSchemaGenerationReason::NoCachedSchemaNormalized,
        })
    }

    async fn normalize_applied_schema(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        selected_schema: &ProductCssSelectorSchema,
        raw: RawExtractedProduct,
    ) -> Result<PreparedSchemaSelection, ScraperError> {
        let default_currency = selected_schema.default_currency.map(Currency::from);
        match self
            .normalization_service
            .normalize(raw.clone(), url.clone(), default_currency)
            .await
        {
            Ok(NormalizationSuccess {
                prepared,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(listing_source_id, url, llm_calls_used)
                    .await?;
                Ok(PreparedSchemaSelection {
                    prepared,
                    raw,
                    default_currency,
                    schema: selected_schema.clone(),
                    fresh_schema: false,
                })
            }
            Err(NormalizationFailure {
                error,
                llm_calls_used,
            }) => {
                self.consume_llm_budget_n_or_err(listing_source_id, url, llm_calls_used)
                    .await?;
                Err(ScraperError::NormalizationError(error))
            }
        }
    }
}

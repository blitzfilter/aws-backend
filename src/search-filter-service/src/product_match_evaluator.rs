use application::error::box_error;
use large_language_model::{
    BatchGenerationOptions, GenerationOptions, LargeLanguageModel, LargeLanguageModelError,
    StructuredGenerationRequest,
};
use localization::Language;
use product_listing_service::ports::ProductListingSearchFilterMatchSource;
use search_filter_core::enhanced_match_reason::EnhancedMatchReason;
use serde::Deserialize;
use std::num::NonZeroUsize;

const MAX_PRODUCT_MATCH_IMAGES: usize = 5;
const PRODUCT_MATCH_SYSTEM_INSTRUCTION: &str = "You are a product matching assistant for an antiques marketplace. Decide whether the product actually matches the requested search description using the product title, description, and optional product images. Return only JSON with a boolean `matches` and, when `matches` is true, a compact user-facing `reason` in the search language. Do not include markdown or extra fields.";

pub(crate) struct ProductListingMatchEvaluationRequest<'a, Key> {
    pub(crate) key: Key,
    pub(crate) product: &'a ProductListingSearchFilterMatchSource,
    pub(crate) search_description: &'a str,
    pub(crate) search_language: Language,
}

pub(crate) struct ProductListingMatchEvaluationResult<Key> {
    pub(crate) key: Key,
    pub(crate) outcome: ProductListingMatchEvaluationOutcome,
}

pub(crate) enum ProductListingMatchEvaluationOutcome {
    Matched(EnhancedMatchReason),
    Rejected,
    RetryableFailure(LargeLanguageModelError),
    PermanentFailure(LargeLanguageModelError),
}

#[derive(Debug, Deserialize)]
struct ProductListingMatchDecision {
    matches: bool,
    #[serde(default)]
    reason: Option<String>,
}

pub(crate) async fn evaluate_product_matches<E, Key>(
    llm: &E,
    evaluations: Vec<ProductListingMatchEvaluationRequest<'_, Key>>,
    max_concurrent_requests: NonZeroUsize,
) -> Vec<ProductListingMatchEvaluationResult<Key>>
where
    E: LargeLanguageModel,
{
    let (keys, requests): (Vec<_>, Vec<_>) = evaluations
        .into_iter()
        .map(|evaluation| {
            (
                evaluation.key,
                product_match_request(
                    evaluation.product,
                    evaluation.search_description,
                    evaluation.search_language,
                ),
            )
        })
        .unzip();
    let results = llm
        .generate_batch::<ProductListingMatchDecision>(
            requests,
            BatchGenerationOptions::new(max_concurrent_requests),
        )
        .await;

    keys.into_iter()
        .zip(results)
        .map(|(key, result)| ProductListingMatchEvaluationResult {
            key,
            outcome: match result.and_then(product_match_reason) {
                Ok(Some(reason)) => ProductListingMatchEvaluationOutcome::Matched(reason),
                Ok(None) => ProductListingMatchEvaluationOutcome::Rejected,
                Err(error) if is_retryable_llm_error(&error) => {
                    ProductListingMatchEvaluationOutcome::RetryableFailure(error)
                }
                Err(error) => ProductListingMatchEvaluationOutcome::PermanentFailure(error),
            },
        })
        .collect()
}

fn product_match_request(
    product: &ProductListingSearchFilterMatchSource,
    search_description: &str,
    search_language: Language,
) -> StructuredGenerationRequest {
    let (title, description) = product_text(product, search_language);
    let prompt = format!(
        "User's search description: {search_description}\nProduct title: {title}\nProduct description: {description}\nSearch language: {}\nReturn the reason in the search language.",
        search_language.format_human_readable(),
    );
    StructuredGenerationRequest {
        operation: large_language_model::LlmOperation::ProductEnhancedSearchDescriptionMatching,
        system_instruction: PRODUCT_MATCH_SYSTEM_INSTRUCTION.to_owned(),
        prompt,
        image_urls: product
            .images
            .iter()
            .take(MAX_PRODUCT_MATCH_IMAGES)
            .map(|image| image.url().clone())
            .collect(),
        response_json_schema: product_match_response_schema(),
        options: GenerationOptions {
            temperature: 0.0,
            max_output_tokens: 256,
            request_timeout: std::time::Duration::from_secs(30),
        },
    }
}

fn product_match_reason(
    decision: ProductListingMatchDecision,
) -> Result<Option<EnhancedMatchReason>, LargeLanguageModelError> {
    if !decision.matches {
        return Ok(None);
    }
    decision
        .reason
        .filter(|reason| !reason.trim().is_empty())
        .map(EnhancedMatchReason::from)
        .map(Some)
        .ok_or_else(|| LargeLanguageModelError::InvalidResponse {
            source: box_error(std::io::Error::other("matched response has no reason")),
        })
}

fn product_match_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "OBJECT",
        "properties": {
            "matches": {"type": "BOOLEAN"},
            "reason": {"type": "STRING"}
        },
        "required": ["matches"]
    })
}

fn product_text(
    product: &ProductListingSearchFilterMatchSource,
    search_language: Language,
) -> (&str, &str) {
    let title = product
        .titles
        .get(&search_language)
        .or_else(|| product.titles.get(&Language::En))
        .map(AsRef::as_ref)
        .or_else(|| {
            product
                .product_title
                .as_ref()
                .map(|title| title.payload.as_ref())
        })
        .unwrap_or("");
    let description = product
        .descriptions
        .get(&search_language)
        .or_else(|| product.descriptions.get(&Language::En))
        .map(AsRef::as_ref)
        .unwrap_or("");
    (title, description)
}

fn is_retryable_llm_error(error: &LargeLanguageModelError) -> bool {
    matches!(
        error,
        LargeLanguageModelError::Timeout { .. }
            | LargeLanguageModelError::Retryable { .. }
            | LargeLanguageModelError::InvalidResponse { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::event_id::EventId;
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use product_listing_core::{
        listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
        product_listing::ProductListingPricing, product_listing_image::ProductListingImage,
        product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
    };
    use product_listing_service::ports::{
        ListingSourceSummary, ProductListingSearchFilterMatchSourceEventKind,
    };
    use url::Url;

    fn product() -> Result<ProductListingSearchFilterMatchSource, url::ParseError> {
        let url = Url::parse("https://example.test/product")?;
        let event_id = EventId::new();
        Ok(ProductListingSearchFilterMatchSource {
            event_id,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: time::OffsetDateTime::UNIX_EPOCH,
            current_event_id: event_id,
            projection_version: 1,
            product_listing_id: product_listing_core::product_listing_id::ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("product-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                slug_id: ListingSourceSlugId::raw("source")
                    .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            },
            source_listing_id: SourceListingId::try_from("product")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            product_title: None,
            product_description: None,
            titles: std::collections::HashMap::new(),
            descriptions: std::collections::HashMap::new(),
            pricing: ProductListingPricing::default(),
            sale_observation: None,
            availability: Some(ListingAvailability::Available),
            lifecycle: ListingLifecycle::Active,
            url: url.clone(),
            view_url: url,
            image: None,
            images: IndexSet::new(),
            embedding: None,
            auction: None,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        })
    }

    #[test]
    fn should_include_description_localized_product_text_and_language_in_prompt()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = product()?;
        product.titles.insert(Language::En, "Brass lamp".into());
        product
            .descriptions
            .insert(Language::En, "From 1920".into());

        let request = product_match_request(&product, "vintage lighting", Language::En);

        assert!(request.prompt.contains("vintage lighting"));
        assert!(request.prompt.contains("Brass lamp"));
        assert!(request.prompt.contains("From 1920"));
        assert!(request.prompt.contains("English"));
        Ok(())
    }

    #[test]
    fn should_include_only_the_first_five_product_images_in_an_enhanced_request()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut product = product()?;
        let image_urls = (0..7)
            .map(|index| Url::parse(&format!("https://example.test/image-{index}.jpg")))
            .collect::<Result<Vec<_>, _>>()?;
        for url in &image_urls {
            product.images.insert(ProductListingImage::new(url.clone()));
        }

        let request = product_match_request(&product, "only paintings", Language::En);

        assert_eq!(
            &image_urls[..MAX_PRODUCT_MATCH_IMAGES],
            request.image_urls.as_slice()
        );
        Ok(())
    }

    #[test]
    fn should_reject_non_matching_response() -> Result<(), LargeLanguageModelError> {
        assert!(
            product_match_reason(ProductListingMatchDecision {
                matches: false,
                reason: Some("not relevant".to_owned()),
            })?
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn should_reject_matched_response_without_reason() {
        let error = product_match_reason(ProductListingMatchDecision {
            matches: true,
            reason: None,
        });

        assert!(matches!(
            error,
            Err(LargeLanguageModelError::InvalidResponse { .. })
        ));
    }

    struct OrderedEvaluator;

    #[async_trait::async_trait]
    impl LargeLanguageModel for OrderedEvaluator {
        async fn generate<Output>(
            &self,
            request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: serde::de::DeserializeOwned + Send,
        {
            let response = if request.prompt.contains("matching request") {
                r#"{"matches":true,"reason":"matches"}"#
            } else {
                r#"{"matches":false}"#
            };
            serde_json::from_str(response).map_err(|source| {
                LargeLanguageModelError::InvalidResponse {
                    source: box_error(source),
                }
            })
        }
    }

    #[tokio::test]
    async fn should_preserve_request_key_order_when_mapping_batch_results()
    -> Result<(), Box<dyn std::error::Error>> {
        let product = product()?;
        let results = evaluate_product_matches(
            &OrderedEvaluator,
            vec![
                ProductListingMatchEvaluationRequest {
                    key: "first",
                    product: &product,
                    search_description: "matching request",
                    search_language: Language::En,
                },
                ProductListingMatchEvaluationRequest {
                    key: "second",
                    product: &product,
                    search_description: "rejected request",
                    search_language: Language::En,
                },
            ],
            NonZeroUsize::MIN,
        )
        .await;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].key, "first");
        assert!(matches!(
            results[0].outcome,
            ProductListingMatchEvaluationOutcome::Matched(_)
        ));
        assert_eq!(results[1].key, "second");
        assert!(matches!(
            results[1].outcome,
            ProductListingMatchEvaluationOutcome::Rejected
        ));
        Ok(())
    }

    #[test]
    fn should_classify_invalid_responses_as_retryable_and_permanent_errors_as_permanent() {
        let invalid_response = LargeLanguageModelError::InvalidResponse {
            source: box_error(std::io::Error::other("invalid response")),
        };
        let permanent = LargeLanguageModelError::Permanent {
            source: box_error(std::io::Error::other("invalid provider request")),
        };

        assert!(is_retryable_llm_error(&invalid_response));
        assert!(!is_retryable_llm_error(&permanent));
    }
}

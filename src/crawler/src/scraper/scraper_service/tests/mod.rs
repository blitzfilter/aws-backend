use listing_source_core::ListingSourceId;
mod bookkeeping;
mod budget;
mod cached_schema_selection;
mod fresh_schema_generation;
mod happy_path;
mod hash_skip;
mod image_evidence;
mod redirect_guard;
mod removed_page;
mod richest_schema_selection;
mod schema_fallback;
mod seed_pages;

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

use crate::scraper::candidate_service::MockScraperCandidateService;
use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::product_schema_service::{
    GeneratedProductSchemas, GeneratedSingleSchema, MockProductListingSchemaService,
    SchemaLlmEvaluation, SchemaLlmEvaluationConfidence, SchemaLlmEvaluationDecision,
};
use crate::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
};
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::normalization::product_normalization_service::{
    MockProductListingNormalizationService, NormalizationFailure, NormalizationSuccess,
    PreparedProduct,
};
use crate::scraper::scraper_service::ScraperService;
use crate::scraper::scraper_service::service::{
    DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE, FetchedHtml, MockHtmlFetcher, ScraperServiceImpl,
};
use crate::spider::classification::url_metadata::{CrawlerDisposition, CrawlerUrlWriteOutcome};
use localization::Language;
use localization::Localized;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use product_listing_normalization::ListingAvailabilityQuickCheck;
use std::sync::Arc;
use time::OffsetDateTime;
use url::Url;

pub(super) fn listing_source_id() -> ListingSourceId {
    ListingSourceId::new()
}

pub(super) fn product_url() -> Url {
    Url::parse("https://example.com/products/123").unwrap()
}

pub(super) fn sample_html() -> String {
    r#"<!DOCTYPE html>
    <html>
    <body>
      <main>
        <span id="product-id">SKU-42</span>
        <h1>Biedermeier Chair</h1>
        <span id="state">In Stock</span>
        <img src="/images/chair-640x640.jpg">
      </main>
    </body>
    </html>"#
        .to_string()
}

pub(super) fn fetch_result(html: String) -> FetchedHtml {
    FetchedHtml::new(html, product_url())
}

pub(super) fn fetch_result_for(html: String, final_url: Url) -> FetchedHtml {
    FetchedHtml::new(html, final_url)
}

pub(super) fn minimal_schema() -> ProductCssSelectorSchema {
    let text_rule = |selector: &str| ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: vec![],
        extract: ExtractionKind::Text,
        cardinality: ExtractionCardinality::First,
    };
    let attr_rule_all = |selector: &str, attr: &str| ExtractionRule {
        selector: CssSelector::from(selector),
        additional_selectors: vec![],
        extract: ExtractionKind::Attribute { name: attr.into() },
        cardinality: ExtractionCardinality::All,
    };

    ProductCssSelectorSchema {
        source_listing_id: Some(text_rule("#product-id")),
        title: text_rule("h1"),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: text_rule("#state"),
        images: attr_rule_all("img", "src"),
        auction_start: None,
        auction_end: None,
        default_currency: None,
        raw_attributes: Default::default(),
    }
}

pub(super) fn listing_source_product_schemas(
    listing_source_id: ListingSourceId,
) -> ListingSourceProductSchema {
    let schema = minimal_schema();
    ListingSourceProductSchema {
        listing_source_id,
        product_schemas: vec![schema],
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    }
}

pub(super) fn generated_schemas(
    schemas: Vec<ProductCssSelectorSchema>,
    confidence: SchemaLlmEvaluationConfidence,
) -> GeneratedProductSchemas {
    GeneratedProductSchemas {
        schemas,
        evaluation: SchemaLlmEvaluation {
            decision: if confidence == SchemaLlmEvaluationConfidence::High {
                SchemaLlmEvaluationDecision::Approve
            } else {
                SchemaLlmEvaluationDecision::NeedsHumanReview
            },
            confidence,
            approved_by_llm: false,
            summary: "Selectors are product-specific.".to_string(),
            risks: Vec::new(),
        },
    }
}

pub(super) fn schema_evaluation(confidence: SchemaLlmEvaluationConfidence) -> SchemaLlmEvaluation {
    SchemaLlmEvaluation {
        decision: if confidence == SchemaLlmEvaluationConfidence::High {
            SchemaLlmEvaluationDecision::Approve
        } else {
            SchemaLlmEvaluationDecision::NeedsHumanReview
        },
        confidence,
        approved_by_llm: false,
        summary: "Selectors are product-specific.".to_string(),
        risks: Vec::new(),
    }
}

pub(super) fn generated_single_product(
    schema: ProductCssSelectorSchema,
    confidence: SchemaLlmEvaluationConfidence,
) -> GeneratedSingleSchema {
    GeneratedSingleSchema::ProductListing {
        schema: Box::new(schema),
        evaluation: schema_evaluation(confidence),
    }
}

pub(super) fn prepared_product(url: Url) -> PreparedProduct {
    let title: Title = "Biedermeier Chair".into();
    PreparedProduct {
        availability: ListingAvailabilityQuickCheck::Resolved(ListingAvailability::Available),
        raw_state: "In Stock".to_string(),
        source_listing_id: SourceListingId::try_from("SKU-42")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        title: Localized::new(Language::De, title),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        url,
        images: vec![],
        auction_start: None,
        auction_end: None,
        raw_attributes: Default::default(),
    }
}

pub(super) fn normalization_success(
    prepared: PreparedProduct,
    llm_calls_used: u32,
) -> NormalizationSuccess {
    NormalizationSuccess {
        prepared,
        llm_calls_used,
    }
}

pub(super) fn normalization_failure(
    error: NormalizationError,
    llm_calls_used: u32,
) -> NormalizationFailure {
    NormalizationFailure {
        error,
        llm_calls_used,
    }
}

/// Crawler disposition changes only after the cron-owned canonical handoff, not inside scraping.
pub(super) fn expect_successful_bookkeeping(
    _: &mut MockScraperCandidateService,
    _: ListingSourceId,
    _: Url,
    _: CrawlerDisposition,
) {
}

pub(super) fn expect_budget_increment(cand_svc: &mut MockScraperCandidateService, times: usize) {
    cand_svc
        .expect_try_increment_listing_source_llm_calls_with_limit()
        .times(times)
        .returning(|_, _, _| Box::pin(async { Ok(true) }));
}

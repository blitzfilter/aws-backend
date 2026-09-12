use crate::CrawlerDomainId;
use listing_source_core::ListingSourceId;
use std::sync::Arc;

use crate::network::policy::is_same_or_www_host;
use crate::spider::classification::url_metadata_repository::UrlMetadataRepository;
use crate::spider::classification::url_pattern_service::{
    UrlPatternService, UrlPatternServiceError,
};
use crate::spider::discovery::website_spider::{
    CrawlDiagnostics, CrawlFailureKind, CrawledPage, Spider, SpiderDiscoveryError,
};
use crate::spider::service::crawl_run_state::CrawlRunState;
use crate::spider::service::product_pattern::ProductListingPattern;
use crate::spider::utils::url::CrawledUrl;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use url::Url;

use crate::spider::classification::url_metadata::UrlClass;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderRunResult {
    pub total_links: usize,
    pub product_urls_count: usize,
    pub product_pattern: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpiderServiceConfig {
    pub db_batch_size: usize,
    pub max_sample_urls: usize,
    pub min_inference_sample_urls: usize,
}

#[derive(Debug, Error)]
pub enum SpiderServiceError {
    #[error(transparent)]
    Discovery(#[from] SpiderDiscoveryError),

    #[error(transparent)]
    UrlPattern(#[from] UrlPatternServiceError),

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    UrlMetadata(
        Box<crate::spider::classification::url_metadata_repository::UrlMetadataRepositoryError>,
    ),

    #[error("Spider crawl emitted no pages for crawl-root URL '{crawl_root_url}'")]
    EmptyCrawl { crawl_root_url: String },

    #[error(
        "Spider crawl emitted only {total_links} page(s) for crawl-root URL '{crawl_root_url}'"
    )]
    TinyCrawl {
        crawl_root_url: String,
        total_links: usize,
    },

    #[error(
        "Spider crawl failed for crawl-root URL '{crawl_root_url}' with diagnostic kind '{kind}' after {total_links} page(s)"
    )]
    DiagnosticCrawlFailure {
        crawl_root_url: String,
        kind: CrawlFailureKind,
        total_links: usize,
        http_status: Option<u16>,
        final_url: Option<String>,
        redirect_url: Option<String>,
        diagnostic_reason: Option<String>,
    },

    #[error(
        "Cannot classify product URL pattern for crawl-root URL '{crawl_root_url}' at stage '{stage}' because the inference sample has {sample_size} URL(s), minimum is {min_sample_size}"
    )]
    InsufficientInferenceSample {
        crawl_root_url: String,
        stage: &'static str,
        sample_size: usize,
        min_sample_size: usize,
    },

    #[error(
        "Cannot classify product URL pattern for crawl-root URL '{crawl_root_url}' at stage '{stage}' because the inference sample is empty"
    )]
    EmptyClassificationSample {
        crawl_root_url: String,
        stage: &'static str,
    },
}

impl Default for SpiderServiceConfig {
    fn default() -> Self {
        Self {
            db_batch_size: 100,
            max_sample_urls: 500,
            min_inference_sample_urls: 20,
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait SpiderService: Send + Sync {
    async fn run(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        _crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError>;
}

pub struct SpiderServiceImpl {
    config: SpiderServiceConfig,
    spider: Box<dyn Spider>,
    pattern_service: Box<dyn UrlPatternService>,
    url_metadata_repository: Arc<dyn UrlMetadataRepository>,
}

impl SpiderServiceImpl {
    pub fn new(
        config: SpiderServiceConfig,
        spider: Box<dyn Spider>,
        pattern_service: Box<dyn UrlPatternService>,
        url_metadata_repository: Arc<dyn UrlMetadataRepository>,
    ) -> Self {
        Self {
            config,
            spider,
            pattern_service,
            url_metadata_repository,
        }
    }

    async fn persist_url_metadata_batch(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pages: &[CrawledPage],
        pattern: &ProductListingPattern,
    ) -> Result<usize, SpiderServiceError> {
        if pages.is_empty() {
            return Ok(0);
        }

        let mut urls = Vec::with_capacity(pages.len());
        let mut classes = Vec::with_capacity(pages.len());

        for page in pages {
            urls.push(page.url.as_url().clone());

            let class_str = classify_url(page.url.as_url().as_str(), pattern).as_str();
            let class = std::str::FromStr::from_str(class_str).unwrap_or(UrlClass::Other);
            classes.push(class);
        }

        if !urls.is_empty() {
            let records = self
                .url_metadata_repository
                .upsert_links_batch(listing_source_id, domain_id, &urls, &classes)
                .await
                .map_err(|error| SpiderServiceError::UrlMetadata(Box::new(error)))?;
            Ok(records.len())
        } else {
            Ok(0)
        }
    }

    async fn process_buffer(
        &self,
        buffer: &mut Vec<CrawledPage>,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: &ProductListingPattern,
    ) -> Result<usize, SpiderServiceError> {
        let count = buffer
            .iter()
            .filter(|p| {
                pattern
                    .as_regex()
                    .is_some_and(|regex| p.url.matches_pattern(regex))
            })
            .count();
        self.persist_url_metadata_batch(listing_source_id, domain_id, buffer, pattern)
            .await?;
        buffer.clear();
        Ok(count)
    }

    #[tracing::instrument(
        name = "spider_classify_and_save_for_stage",
        skip(self, state),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url, stage)
    )]
    async fn classify_and_save_for_stage(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        stage: &'static str,
    ) -> Result<(), SpiderServiceError> {
        if state.inference_sample.len() < self.config.min_inference_sample_urls {
            return Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: crawl_root_url.to_string(),
                stage,
                sample_size: state.inference_sample.len(),
                min_sample_size: self.config.min_inference_sample_urls,
            });
        }

        state.pattern = self
            .pattern_service
            .classify_and_save(
                listing_source_id,
                domain_id,
                crawl_root_url,
                &state.inference_sample,
            )
            .await
            .map(|pattern| {
                pattern
                    .map(ProductListingPattern::from)
                    .unwrap_or(ProductListingPattern::Unknown)
            })?;

        if state.pattern.is_unknown() {
            warn!(stage, "Found no product URL pattern");
        }

        Ok(())
    }

    #[tracing::instrument(
        name = "spider_maybe_classify_at_threshold",
        skip(self, state),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url, classify_threshold)
    )]
    async fn maybe_classify_at_threshold(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<(), SpiderServiceError> {
        if !state.classification_done && state.total_crawled >= classify_threshold {
            debug!(
                url_count = state.inference_sample.len(),
                "Threshold reached, requesting product URL pattern"
            );

            self.classify_and_save_for_stage(
                state,
                listing_source_id,
                domain_id,
                crawl_root_url,
                "threshold",
            )
            .await?;

            state.classification_done = true;
            state.pattern_loaded_from_store = false;
        }

        Ok(())
    }

    async fn flush_batch_if_needed(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), SpiderServiceError> {
        if state.classification_done && state.page_buffer.len() >= self.config.db_batch_size {
            state.products_found += self
                .process_buffer(
                    &mut state.page_buffer,
                    listing_source_id,
                    domain_id,
                    &state.pattern,
                )
                .await?;
        }
        Ok(())
    }

    #[tracing::instrument(name = "spider_log_progress", skip(self, state))]
    fn log_progress(&self, state: &CrawlRunState) {
        if state.total_crawled.is_multiple_of(1000) {
            info!(
                total_crawled = state.total_crawled,
                products_found = state.products_found,
                "Crawl progress"
            );
        }
    }

    #[tracing::instrument(
        name = "spider_classify_at_end_if_needed",
        skip(self, state),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url)
    )]
    async fn classify_at_end_if_needed(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
    ) -> Result<(), SpiderServiceError> {
        if !state.classification_done && !state.page_buffer.is_empty() {
            info!(
                url_count = state.inference_sample.len(),
                "Threshold not reached, classifying collected URLs"
            );

            self.classify_and_save_for_stage(
                state,
                listing_source_id,
                domain_id,
                crawl_root_url,
                "end_of_crawl",
            )
            .await?;

            state.classification_done = true;
        }
        Ok(())
    }

    #[tracing::instrument(
        name = "spider_reclassify_if_persisted_pattern_failed",
        skip(self, state),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url)
    )]
    async fn reclassify_if_persisted_pattern_failed(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
    ) -> Result<(), SpiderServiceError> {
        if state.pattern_loaded_from_store
            && state.products_found == 0
            && !state.inference_sample.is_empty()
        {
            warn!("Persisted product URL pattern did not match crawl results, reclassifying");

            self.classify_and_save_for_stage(
                state,
                listing_source_id,
                domain_id,
                crawl_root_url,
                "refresh",
            )
            .await?;
        }

        Ok(())
    }

    async fn flush_remaining_pages(
        &self,
        state: &mut CrawlRunState,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), SpiderServiceError> {
        if !state.page_buffer.is_empty() {
            state.products_found += self
                .process_buffer(
                    &mut state.page_buffer,
                    listing_source_id,
                    domain_id,
                    &state.pattern,
                )
                .await?;
        }
        Ok(())
    }

    #[tracing::instrument(
        name = "spider_mark_as_crawled_best_effort",
        skip(self),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url)
    )]
    async fn mark_as_crawled_best_effort(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
    ) {
        if let Err(error) = self
            .pattern_service
            .mark_as_crawled(listing_source_id, domain_id)
            .await
        {
            warn!(error = ?error, "Failed to mark ListingSource as crawled");
        }
    }

    #[tracing::instrument(
        name = "spider_run_locked",
        skip(self),
        fields(
            listing_source_id = %listing_source_id,
            domain_id = %domain_id,
            crawl_root_url = %crawl_root_url,
            classify_threshold
        )
    )]
    async fn run_locked(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        let configured_root = Url::parse(crawl_root_url).map_err(|_| {
            SpiderServiceError::Discovery(SpiderDiscoveryError::Discovery(
                "configured crawler domain URL is invalid".to_string(),
            ))
        })?;
        let mut crawl = self.spider.crawl(crawl_root_url).await?;

        let initial_pattern = self
            .pattern_service
            .load_pattern_for_domain(listing_source_id, domain_id)
            .await?;
        let mut state = CrawlRunState::new(initial_pattern);

        if state.pattern_loaded_from_store {
            debug!("Loaded persisted product URL pattern");
        }

        while let Some(page) = crawl.recv().await? {
            if !is_same_or_www_host(&configured_root, page.url.as_url()) {
                debug!(url = %page.url, configured_root = %configured_root, "Ignoring discovered URL outside configured crawler domain");
                continue;
            }
            state.total_crawled += 1;

            if state.inference_sample.len() < self.config.max_sample_urls {
                state.inference_sample.push(page.url.to_string());
            }

            state.page_buffer.push(page.clone());

            self.maybe_classify_at_threshold(
                &mut state,
                listing_source_id,
                domain_id,
                crawl_root_url,
                classify_threshold,
            )
            .await?;

            self.flush_batch_if_needed(&mut state, listing_source_id, domain_id)
                .await?;
            self.log_progress(&state);
        }

        let diagnostics = crawl.completion().await?;
        if let Some(error) =
            diagnostic_failure_error(crawl_root_url, state.total_crawled, &diagnostics)
                .or_else(|| crawl_size_failure_error(crawl_root_url, state.total_crawled))
        {
            return Err(error);
        }

        self.classify_at_end_if_needed(&mut state, listing_source_id, domain_id, crawl_root_url)
            .await?;
        self.reclassify_if_persisted_pattern_failed(
            &mut state,
            listing_source_id,
            domain_id,
            crawl_root_url,
        )
        .await?;
        self.flush_remaining_pages(&mut state, listing_source_id, domain_id)
            .await?;

        let product_pattern = state
            .pattern
            .as_regex()
            .map(|regex| regex.as_str().to_string());

        self.mark_as_crawled_best_effort(listing_source_id, domain_id, crawl_root_url)
            .await;

        info!(
            total_crawled = state.total_crawled,
            product_urls_count = state.products_found,
            product_pattern_known = product_pattern.is_some(),
            classification_done = state.classification_done,
            "Crawl completed successfully"
        );

        Ok(SpiderRunResult {
            total_links: state.total_crawled,
            product_urls_count: state.products_found,
            product_pattern,
        })
    }
}

#[async_trait::async_trait]
impl SpiderService for SpiderServiceImpl {
    #[tracing::instrument(
        name = "spider_run",
        skip(self),
        fields(
            listing_source_id = %listing_source_id,
            domain_id = %domain_id,
            crawl_root_url = %crawl_root_url,
            classify_threshold
        )
    )]
    async fn run(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        debug!("Starting crawl");

        self.run_locked(
            listing_source_id,
            domain_id,
            crawl_root_url,
            classify_threshold,
        )
        .await
    }
}

fn classify_url(url: &str, product_pattern: &ProductListingPattern) -> UrlClass {
    let crawled = match Url::parse(url) {
        Ok(parsed) => CrawledUrl::new(parsed),
        Err(_) => return UrlClass::Other,
    };

    crawled.classify(product_pattern.as_regex())
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn should_classify_product_when_pattern_matches_for_type() {
        let pattern = ProductListingPattern::Known(Regex::new(r"/product/").unwrap());

        let class = classify_url("https://example.com/product/42", &pattern);

        assert_eq!(class, UrlClass::ProductListing);
    }

    #[test]
    fn should_classify_imprint_when_url_contains_legal_keywords_for_type() {
        let pattern = ProductListingPattern::Unknown;

        let class = classify_url("https://example.com/impressum", &pattern);

        assert_eq!(class, UrlClass::Imprint);
    }

    #[test]
    fn should_classify_category_when_url_contains_category_keywords_for_type() {
        let pattern = ProductListingPattern::Unknown;

        let class = classify_url("https://example.com/collections/modern", &pattern);

        assert_eq!(class, UrlClass::Category);
    }

    #[test]
    fn should_classify_other_when_url_does_not_match_any_rule_for_type() {
        let pattern = ProductListingPattern::Unknown;

        let class = classify_url("https://example.com/random-page", &pattern);

        assert_eq!(class, UrlClass::Other);
    }

    #[test]
    fn should_map_known_db_value_when_loading_url_class() {
        let class = UrlClass::from_db("category");

        assert_eq!(class, UrlClass::Category);
    }

    #[test]
    fn should_map_unknown_db_value_to_other_when_loading_url_class() {
        let class = UrlClass::from_db("unknown-value");

        assert_eq!(class, UrlClass::Other);
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::spider::classification::url_metadata_repository::MockUrlMetadataRepository;
    use crate::spider::classification::url_pattern_service::MockUrlPatternService;
    use crate::spider::discovery::website_spider::{
        CrawlDiagnostics, CrawlFailureKind, MockSpider, SpiderCrawl,
    };
    use regex::Regex;

    fn setup_mock_url_repo(
        mock: &mut MockUrlMetadataRepository,
        call_count: usize,
        expected_domain_id: CrawlerDomainId,
    ) {
        mock.expect_upsert_links_batch()
            .times(call_count)
            .withf(move |_, domain_id, _, _| *domain_id == expected_domain_id)
            .returning(|_, _, _, _| Box::pin(async move { Ok(Vec::new()) }));
    }

    fn setup_mock_mark_as_crawled(mock: &mut MockUrlPatternService, _crawl_root_url: &'static str) {
        mock.expect_mark_as_crawled()
            .with(mockall::predicate::always(), mockall::predicate::always())
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(()) }));
    }

    fn setup_mock_crawl<I, S>(mock: &mut MockSpider, crawl_root_url: &'static str, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        setup_mock_crawl_with_diagnostics(mock, crawl_root_url, paths, CrawlDiagnostics::default());
    }

    fn setup_mock_crawl_with_diagnostics<I, S>(
        mock: &mut MockSpider,
        crawl_root_url: &'static str,
        paths: I,
        diagnostics: CrawlDiagnostics,
    ) where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let paths: Vec<String> = paths.into_iter().map(Into::into).collect();
        mock.expect_crawl()
            .with(mockall::predicate::eq(crawl_root_url))
            .returning(move |_| {
                let pages = paths
                    .iter()
                    .map(|path| CrawledPage {
                        url: CrawledUrl::new(
                            Url::parse(&format!("https://example.com{path}")).unwrap(),
                        ),
                    })
                    .collect();
                let diagnostics = diagnostics.clone();
                Box::pin(async move { Ok(SpiderCrawl::fixture(pages, Some(diagnostics))) })
            });
    }

    fn one_product_and_listing_pages() -> Vec<String> {
        let mut paths = vec!["/product/1".to_string()];
        paths.extend((1..20).map(|i| format!("/about/{i}")));
        paths
    }

    fn item_pages() -> Vec<String> {
        (1..=20).map(|i| format!("/item/{i}")).collect()
    }

    #[tokio::test]
    async fn should_run_spider_and_classify_urls() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            one_product_and_listing_pages(),
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));

        mock_pattern_service
            .expect_classify_and_save()
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 20)
            .await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
        assert_eq!(run_result.total_links, 20);
    }

    #[tokio::test]
    async fn should_classify_at_end_if_threshold_not_reached() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            one_product_and_listing_pages(),
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));

        // It should classify at the end because threshold is above the crawl size.
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _| {
                Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) })
            });

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 25)
            .await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 1);
    }

    #[tokio::test]
    async fn should_reclassify_if_persisted_pattern_fails() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, item_pages());

        // Persisted pattern expects /product/
        mock_pattern_service
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));

        // Reclassification gives the correct /item/ pattern
        mock_pattern_service
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));

        setup_mock_mark_as_crawled(&mut mock_pattern_service, crawl_root_url);

        setup_mock_url_repo(&mut mock_url_repo, 1, domain_id);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 25)
            .await;
        assert!(result.is_ok());
        let run_result = result.unwrap();
        assert_eq!(run_result.product_urls_count, 20);
    }

    #[tokio::test]
    async fn should_return_empty_crawl_error_without_classifying_or_marking_crawled() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        mock_spider
            .expect_crawl()
            .with(mockall::predicate::eq(crawl_root_url))
            .returning(|_| {
                Box::pin(async {
                    Ok(SpiderCrawl::fixture(
                        Vec::new(),
                        Some(CrawlDiagnostics::default()),
                    ))
                })
            });

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::EmptyCrawl { crawl_root_url: url }) if url == crawl_root_url
        ));
    }

    #[tokio::test]
    async fn should_return_tiny_crawl_error_for_one_url_without_classifying_or_marking_crawled() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, vec!["/"]);

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::TinyCrawl {
                crawl_root_url: url,
                total_links: 1,
            }) if url == crawl_root_url
        ));
    }

    #[tokio::test]
    async fn should_return_insufficient_inference_sample_error_for_two_url_crawl() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(
            &mut mock_spider,
            crawl_root_url,
            vec!["/collections", "/about"],
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: url,
                stage: "end_of_crawl",
                sample_size: 2,
                min_sample_size: 20,
            }) if url == crawl_root_url
        ));
    }

    #[tokio::test]
    async fn should_return_diagnostic_crawl_failure_for_rate_limited_tiny_crawl() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            vec!["/"],
            CrawlDiagnostics {
                failure_kind: Some(CrawlFailureKind::RateLimited),
                http_status: Some(429),
                final_url: Some("https://example.com/".to_string()),
                redirect_url: None,
                diagnostic_reason: Some("canonical_non_success_status".to_string()),
            },
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));

        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::DiagnosticCrawlFailure {
                kind: CrawlFailureKind::RateLimited,
                total_links: 1,
                http_status: Some(429),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn should_keep_insufficient_inference_sample_when_diagnostic_has_multiple_urls() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl_with_diagnostics(
            &mut mock_spider,
            crawl_root_url,
            vec!["/collections", "/about"],
            CrawlDiagnostics {
                failure_kind: Some(CrawlFailureKind::JavascriptRequired),
                diagnostic_reason: Some("few_links_and_app_shell_markers".to_string()),
                ..CrawlDiagnostics::default()
            },
        );

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(None) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::InsufficientInferenceSample { sample_size: 2, .. })
        ));
    }

    #[tokio::test]
    async fn should_return_insufficient_inference_sample_error_for_refresh_with_small_sample() {
        let mut mock_spider = MockSpider::new();
        let mut mock_pattern_service = MockUrlPatternService::new();
        let mut mock_url_repo = MockUrlMetadataRepository::new();

        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let crawl_root_url = "https://example.com";

        setup_mock_crawl(&mut mock_spider, crawl_root_url, vec!["/item/1", "/item/2"]);

        mock_pattern_service
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/product/").unwrap())) }));
        mock_pattern_service.expect_classify_and_save().times(0);
        mock_pattern_service.expect_mark_as_crawled().times(0);
        mock_url_repo.expect_upsert_links_batch().times(0);

        let service = SpiderServiceImpl::new(
            SpiderServiceConfig::default(),
            Box::new(mock_spider),
            Box::new(mock_pattern_service),
            Arc::new(mock_url_repo),
        );

        let result = service
            .run(&listing_source_id, &domain_id, crawl_root_url, 10)
            .await;

        assert!(matches!(
            result,
            Err(SpiderServiceError::InsufficientInferenceSample {
                crawl_root_url: url,
                stage: "refresh",
                sample_size: 2,
                min_sample_size: 20,
            }) if url == crawl_root_url
        ));
    }
}

fn diagnostic_failure_error(
    crawl_root_url: &str,
    total_links: usize,
    diagnostics: &CrawlDiagnostics,
) -> Option<SpiderServiceError> {
    let kind = diagnostics.failure_kind?;
    if total_links > 1 {
        return None;
    }
    Some(SpiderServiceError::DiagnosticCrawlFailure {
        crawl_root_url: crawl_root_url.to_string(),
        kind,
        total_links,
        http_status: diagnostics.http_status,
        final_url: diagnostics.final_url.clone(),
        redirect_url: diagnostics.redirect_url.clone(),
        diagnostic_reason: diagnostics.diagnostic_reason.clone(),
    })
}

fn crawl_size_failure_error(
    crawl_root_url: &str,
    total_links: usize,
) -> Option<SpiderServiceError> {
    match total_links {
        0 => Some(SpiderServiceError::EmptyCrawl {
            crawl_root_url: crawl_root_url.to_string(),
        }),
        1 => Some(SpiderServiceError::TinyCrawl {
            crawl_root_url: crawl_root_url.to_string(),
            total_links,
        }),
        _ => None,
    }
}

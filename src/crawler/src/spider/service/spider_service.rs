use crate::CrawlerDomainId;
use futures::FutureExt;
use listing_source_core::ListingSourceId;
use std::{future::Future, panic::AssertUnwindSafe, sync::Arc};
use tokio::sync::watch;

use crate::network::policy::is_same_or_www_host;
use crate::spider::classification::url_metadata_repository::UrlMetadataRepository;
use crate::spider::classification::url_pattern_service::{
    UrlPatternService, UrlPatternServiceError,
};
use crate::spider::discovery::website_spider::{
    CrawlDiagnostics, CrawlFailureKind, CrawlIncompleteError, CrawledPage, Spider,
    SpiderDiscoveryError,
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
    #[error("Spider crawl cancelled")]
    Cancelled,

    #[error("Spider URL upsert returned {actual} records for {expected} URLs")]
    UrlUpsertCountMismatch { expected: usize, actual: usize },

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
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError>;

    /// True or sender loss stops work. Once a checkpoint starts, its actual result wins.
    /// Callers must retain and await this future; abort/drop is not a completed drain.
    async fn run_until(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
        stop: watch::Receiver<bool>,
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
        stop: &watch::Receiver<bool>,
    ) -> Result<usize, SpiderServiceError> {
        if pages.is_empty() {
            return Ok(0);
        }

        let mut urls = Vec::with_capacity(pages.len());
        let mut classes = Vec::with_capacity(pages.len());

        for page in pages {
            ensure_running(stop)?;
            urls.push(page.url.as_url().clone());

            let class_str = classify_url(page.url.as_url().as_str(), pattern).as_str();
            let class = std::str::FromStr::from_str(class_str).unwrap_or(UrlClass::Other);
            classes.push(class);
        }

        if !urls.is_empty() {
            ensure_running(stop)?;
            let records = self
                .url_metadata_repository
                .upsert_links_batch(listing_source_id, domain_id, &urls, &classes)
                .await
                .map_err(|error| SpiderServiceError::UrlMetadata(Box::new(error)))?;
            if records.len() != urls.len() {
                return Err(SpiderServiceError::UrlUpsertCountMismatch {
                    expected: urls.len(),
                    actual: records.len(),
                });
            }
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
        stop: &watch::Receiver<bool>,
    ) -> Result<usize, SpiderServiceError> {
        let count = buffer.iter().try_fold(0, |count, page| {
            ensure_running(stop)?;
            let matches = pattern
                .as_regex()
                .is_some_and(|regex| page.url.matches_pattern(regex));
            Ok::<_, SpiderServiceError>(count + usize::from(matches))
        })?;
        self.persist_url_metadata_batch(listing_source_id, domain_id, buffer, pattern, stop)
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
        stop: &watch::Receiver<bool>,
    ) -> Result<(), SpiderServiceError> {
        if state.classification_done && state.page_buffer.len() >= self.config.db_batch_size {
            state.products_found += self
                .process_buffer(
                    &mut state.page_buffer,
                    listing_source_id,
                    domain_id,
                    &state.pattern,
                    stop,
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
        stop: &watch::Receiver<bool>,
    ) -> Result<(), SpiderServiceError> {
        if !state.page_buffer.is_empty() {
            state.products_found += self
                .process_buffer(
                    &mut state.page_buffer,
                    listing_source_id,
                    domain_id,
                    &state.pattern,
                    stop,
                )
                .await?;
        }
        Ok(())
    }

    #[tracing::instrument(
        name = "spider_mark_as_crawled",
        skip(self),
        fields(listing_source_id = %listing_source_id, crawl_root_url = %crawl_root_url)
    )]
    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
    ) -> Result<(), SpiderServiceError> {
        self.pattern_service
            .mark_as_crawled(listing_source_id, domain_id)
            .await?;
        Ok(())
    }

    #[tracing::instrument(
        name = "spider_run_locked",
        skip(self, stop),
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
        stop: watch::Receiver<bool>,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        ensure_running(&stop)?;
        let configured_root = Url::parse(crawl_root_url).map_err(|_| {
            SpiderServiceError::Discovery(SpiderDiscoveryError::Discovery(
                "configured crawler domain URL is invalid".to_string(),
            ))
        })?;
        let mut crawl = before_stop(&stop, || self.spider.crawl(crawl_root_url)).await?;

        // Keep ownership outside the unwind boundary so even a service panic joins wrappers
        // before the runtime contains it. No business work is spawned by this service.
        let outcome = AssertUnwindSafe(async {
            let initial_pattern = before_stop(
                &stop,
                || self.pattern_service.load_pattern_for_domain(listing_source_id, domain_id),
            )
            .await?;
            let mut state = CrawlRunState::new(initial_pattern);

            if state.pattern_loaded_from_store {
                debug!("Loaded persisted product URL pattern");
            }

            while let Some(page) = before_stop(&stop, || crawl.recv()).await? {
                ensure_running(&stop)?;
                if !is_same_or_www_host(&configured_root, page.url.as_url()) {
                    debug!(url = %page.url, configured_root = %configured_root, "Ignoring discovered URL outside configured crawler domain");
                    continue;
                }
                state.total_crawled += 1;

                if state.inference_sample.len() < self.config.max_sample_urls {
                    state.inference_sample.push(page.url.to_string());
                }

                state.page_buffer.push(page.clone());

                before_stop(
                    &stop,
                    || self.maybe_classify_at_threshold(
                        &mut state,
                        listing_source_id,
                        domain_id,
                        crawl_root_url,
                        classify_threshold,
                    ),
                )
                .await?;

                before_stop(
                    &stop,
                    || self.flush_batch_if_needed(&mut state, listing_source_id, domain_id, &stop),
                )
                .await?;
                self.log_progress(&state);
            }

            let diagnostics = before_stop(&stop, || crawl.completion()).await?;
            if let Some(error) =
                diagnostic_failure_error(crawl_root_url, state.total_crawled, &diagnostics)
                    .or_else(|| crawl_size_failure_error(crawl_root_url, state.total_crawled))
            {
                return Err(error);
            }

            before_stop(
                &stop,
                || self.classify_at_end_if_needed(&mut state, listing_source_id, domain_id, crawl_root_url),
            )
            .await?;
            before_stop(
                &stop,
                || self.reclassify_if_persisted_pattern_failed(
                    &mut state,
                    listing_source_id,
                    domain_id,
                    crawl_root_url,
                ),
            )
            .await?;
            before_stop(
                &stop,
                || self.flush_remaining_pages(&mut state, listing_source_id, domain_id, &stop),
            )
            .await?;

            let product_pattern = state
                .pattern
                .as_regex()
                .map(|regex| regex.as_str().to_string());

            ensure_running(&stop)?;
            // Never cancel an issued checkpoint: a dropped database future cannot tell us
            // whether it committed. Earlier URL batches may persist and remain retryable.
            self.mark_as_crawled(listing_source_id, domain_id, crawl_root_url)
                .await?;

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
        })
        .catch_unwind()
        .await;

        match outcome {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => {
                if let Err(cleanup) = crawl.cancel_and_join().await
                    && !expected_discovery_cancellation(&cleanup)
                {
                    if matches!(error, SpiderServiceError::Cancelled) {
                        return Err(cleanup.into());
                    }
                    warn!("Crawl cleanup also failed; preserving original service error");
                }
                Err(error)
            }
            Err(payload) => {
                if let Err(cleanup) = crawl.cancel_and_join().await
                    && !expected_discovery_cancellation(&cleanup)
                {
                    warn!("Crawl cleanup also failed after service panic");
                }
                std::panic::resume_unwind(payload)
            }
        }
    }
}

#[async_trait::async_trait]
impl SpiderService for SpiderServiceImpl {
    async fn run(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        let (never_stop, stop) = watch::channel(false);
        let result = self
            .run_until(
                listing_source_id,
                domain_id,
                crawl_root_url,
                classify_threshold,
                stop,
            )
            .await;
        drop(never_stop);
        result
    }

    #[tracing::instrument(
        name = "spider_run",
        skip(self, stop),
        fields(
            listing_source_id = %listing_source_id,
            domain_id = %domain_id,
            crawl_root_url = %crawl_root_url,
            classify_threshold
        )
    )]
    async fn run_until(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        classify_threshold: usize,
        stop: watch::Receiver<bool>,
    ) -> Result<SpiderRunResult, SpiderServiceError> {
        debug!("Starting crawl");

        self.run_locked(
            listing_source_id,
            domain_id,
            crawl_root_url,
            classify_threshold,
            stop,
        )
        .await
    }
}

fn ensure_running(stop: &watch::Receiver<bool>) -> Result<(), SpiderServiceError> {
    if *stop.borrow() || stop.has_changed().is_err() {
        Err(SpiderServiceError::Cancelled)
    } else {
        Ok(())
    }
}

async fn before_stop<T, E, F>(
    stop: &watch::Receiver<bool>,
    work: impl FnOnce() -> F,
) -> Result<T, SpiderServiceError>
where
    F: Future<Output = Result<T, E>>,
    SpiderServiceError: From<E>,
{
    ensure_running(stop)?;
    let mut stop = stop.clone();
    tokio::select! {
        biased;
        _ = async {
            while ensure_running(&stop).is_ok() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
        } => Err(SpiderServiceError::Cancelled),
        result = async { work().await } => result.map_err(Into::into),
    }
}

fn expected_discovery_cancellation(error: &SpiderDiscoveryError) -> bool {
    matches!(
        error,
        SpiderDiscoveryError::Incomplete(CrawlIncompleteError::Cancelled)
    )
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
    use crate::spider::classification::url_metadata::CrawlerDisposition;
    use crate::spider::classification::url_metadata_repository::{
        MockUrlMetadataRepository, SpiderUrlRecord, UrlMetadataRepositoryError,
    };
    use crate::spider::classification::url_pattern_service::MockUrlPatternService;
    use crate::spider::discovery::website_spider::{
        CrawlDiagnostics, CrawlFailureKind, MockSpider, SpiderCrawl,
    };
    use regex::Regex;
    use rstest::rstest;
    use std::collections::BTreeSet;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use std::time::Duration;
    use tokio::sync::Notify;

    fn stored_urls(
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        urls: &[Url],
        classes: &[UrlClass],
    ) -> Vec<SpiderUrlRecord> {
        urls.iter()
            .zip(classes)
            .map(|(url, class)| SpiderUrlRecord {
                listing_source_id: *listing_source_id,
                domain_id: *domain_id,
                url: url.clone(),
                url_class: *class,
                disposition: CrawlerDisposition::Active,
                last_scraped_hash: None,
                last_scraped: None,
                created: time::OffsetDateTime::UNIX_EPOCH,
                updated: time::OffsetDateTime::UNIX_EPOCH,
            })
            .collect()
    }

    fn setup_mock_url_repo(
        mock: &mut MockUrlMetadataRepository,
        call_count: usize,
        expected_domain_id: CrawlerDomainId,
    ) {
        mock.expect_upsert_links_batch()
            .times(call_count)
            .withf(move |_, domain_id, _, _| *domain_id == expected_domain_id)
            .returning(|source, domain, urls, classes| {
                let records = stored_urls(source, domain, urls, classes);
                Box::pin(async move { Ok(records) })
            });
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

    const ROOT: &str = "https://example.com";

    fn mock_service(
        spider: MockSpider,
        patterns: MockUrlPatternService,
        urls: MockUrlMetadataRepository,
        db_batch_size: usize,
    ) -> SpiderServiceImpl {
        SpiderServiceImpl::new(
            SpiderServiceConfig {
                db_batch_size,
                ..Default::default()
            },
            Box::new(spider),
            Box::new(patterns),
            Arc::new(urls),
        )
    }

    fn load_known_pattern(patterns: &mut MockUrlPatternService) {
        patterns
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));
    }

    fn load_and_refresh_known_pattern(patterns: &mut MockUrlPatternService) {
        load_known_pattern(patterns);
        // Existing flow refreshes before counting the final, still-buffered URLs.
        patterns
            .expect_classify_and_save()
            .times(1)
            .returning(|_, _, _, _| Box::pin(async { Ok(Some(Regex::new(r"/item/").unwrap())) }));
    }

    fn fixture_pages(count: usize) -> Vec<CrawledPage> {
        (1..=count)
            .map(|i| CrawledPage {
                url: CrawledUrl::new(Url::parse(&format!("{ROOT}/item/{i}")).unwrap()),
            })
            .collect()
    }

    async fn bounded<F: Future>(work: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(2), work)
            .await
            .expect("fake-only service operation must finish")
    }

    struct FlagOnDrop(Arc<AtomicBool>);

    impl Drop for FlagOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[derive(Clone, Default)]
    struct Gate {
        started: Arc<Notify>,
        release: Arc<Notify>,
        dropped: Arc<AtomicBool>,
    }

    impl Gate {
        async fn wait(&self) {
            let _on_drop = FlagOnDrop(self.dropped.clone());
            self.started.notify_one();
            self.release.notified().await;
        }
    }

    async fn stop_at_gate<F: Future>(
        work: F,
        sender: watch::Sender<bool>,
        gate: &Gate,
    ) -> F::Output {
        let (result, ()) = bounded(async {
            tokio::join!(work, async {
                gate.started.notified().await;
                sender.send(true).unwrap();
            })
        })
        .await;
        assert!(gate.dropped.load(Ordering::SeqCst));
        result
    }

    // Current-thread tests keep the backpressured fixture wrappers alive while the
    // failing service operation is polled. Only awaiting their joins can yield here.
    async fn finish_failure_after_join_wait<F: Future>(work: F, gate: &Gate) -> F::Output {
        tokio::pin!(work);
        bounded(async {
            tokio::select! {
                _ = &mut work => panic!("service finished before fake operation started"),
                _ = gate.started.notified() => {}
            }
            gate.release.notify_one();
            assert!(
                futures::poll!(&mut work).is_pending(),
                "early exit must await wrapper joins"
            );
            assert!(
                gate.dropped.load(Ordering::SeqCst),
                "operation must have exited before join wait"
            );
            work.await
        })
        .await
    }

    #[rstest]
    #[case::initially_stopped(true)]
    #[case::sender_lost(false)]
    #[tokio::test]
    async fn should_do_no_work_when_stop_is_initially_terminal(#[case] initially_stopped: bool) {
        let (sender, stop) = watch::channel(initially_stopped);
        if !initially_stopped {
            drop(sender);
        }
        let service = mock_service(
            MockSpider::new(),
            MockUrlPatternService::new(),
            MockUrlMetadataRepository::new(),
            100,
        );
        let result = service
            .run_until(
                &ListingSourceId::new(),
                &CrawlerDomainId::new(),
                "not even a URL",
                20,
                stop,
            )
            .await;
        assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
    }

    #[tokio::test]
    async fn should_cancel_pending_root_setup_without_pattern_or_storage_work() {
        let gate = Gate::default();
        let root_gate = gate.clone();
        let mut spider = MockSpider::new();
        spider.expect_crawl().times(1).return_once(move |_| {
            Box::pin(async move {
                root_gate.wait().await;
                Ok(SpiderCrawl::fixture(
                    Vec::new(),
                    Some(CrawlDiagnostics::default()),
                ))
            })
        });
        let service = mock_service(
            spider,
            MockUrlPatternService::new(),
            MockUrlMetadataRepository::new(),
            100,
        );
        let (sender, stop) = watch::channel(false);
        let result = stop_at_gate(
            service.run_until(
                &ListingSourceId::new(),
                &CrawlerDomainId::new(),
                ROOT,
                20,
                stop,
            ),
            sender,
            &gate,
        )
        .await;
        assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn should_join_new_crawl_without_loading_pattern_when_root_setup_returns_after_stop() {
        let (sender, stop) = watch::channel(false);
        let mut spider = MockSpider::new();
        spider.expect_crawl().times(1).return_once(move |_| {
            Box::pin(async move {
                let crawl =
                    SpiderCrawl::fixture(fixture_pages(100), Some(CrawlDiagnostics::default()));
                sender.send(true).unwrap();
                Ok(crawl)
            })
        });
        let service = mock_service(
            spider,
            MockUrlPatternService::new(),
            MockUrlMetadataRepository::new(),
            100,
        );
        let source = ListingSourceId::new();
        let domain = CrawlerDomainId::new();
        let work = service.run_until(&source, &domain, ROOT, 20, stop);
        tokio::pin!(work);
        assert!(
            futures::poll!(&mut work).is_pending(),
            "new crawl must be joined, not discarded"
        );
        assert!(matches!(
            bounded(work).await,
            Err(SpiderServiceError::Cancelled)
        ));
    }

    #[rstest]
    #[case::requested_stop(false)]
    #[case::sender_lost(true)]
    #[tokio::test(flavor = "current_thread")]
    async fn should_cancel_page_wait_and_join_wrappers_without_classifying(
        #[case] lose_sender: bool,
    ) {
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        let loaded = Arc::new(AtomicBool::new(false));
        let load_finished = loaded.clone();
        patterns
            .expect_load_pattern_for_domain()
            .times(1)
            .returning(move |_, _| {
                load_finished.store(true, Ordering::SeqCst);
                Box::pin(async { Ok(None) })
            });
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let (sender, stop) = watch::channel(false);
        let source = ListingSourceId::new();
        let domain = CrawlerDomainId::new();
        let work = service.run_until(&source, &domain, ROOT, 20, stop);
        tokio::pin!(work);
        assert!(futures::poll!(&mut work).is_pending());
        assert!(loaded.load(Ordering::SeqCst));
        // No yield yet: fixture tasks have not emitted a page. Service waits in recv.
        if lose_sender {
            drop(sender);
        } else {
            sender.send(true).unwrap();
        }
        assert!(
            futures::poll!(&mut work).is_pending(),
            "stop must wait for wrapper joins"
        );
        assert!(matches!(
            bounded(work).await,
            Err(SpiderServiceError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn should_cancel_pattern_load_without_starting_classification() {
        let gate = Gate::default();
        let load_gate = gate.clone();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        patterns
            .expect_load_pattern_for_domain()
            .times(1)
            .return_once(move |_, _| {
                Box::pin(async move {
                    load_gate.wait().await;
                    Ok(None)
                })
            });
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let (sender, stop) = watch::channel(false);
        let result = stop_at_gate(
            service.run_until(
                &ListingSourceId::new(),
                &CrawlerDomainId::new(),
                ROOT,
                20,
                stop,
            ),
            sender,
            &gate,
        )
        .await;
        assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
    }

    #[rstest]
    #[case::threshold("threshold", 20)]
    #[case::end_of_crawl("end", 25)]
    #[case::refresh("refresh", 25)]
    #[tokio::test]
    async fn should_cancel_pattern_work_without_writing_urls_or_checkpoint(
        #[case] stage: &str,
        #[case] threshold: usize,
    ) {
        let gate = Gate::default();
        let classify_gate = gate.clone();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        let refresh = stage == "refresh";
        patterns
            .expect_load_pattern_for_domain()
            .returning(move |_, _| {
                Box::pin(async move { Ok(refresh.then(|| Regex::new(r"/product/").unwrap())) })
            });
        patterns
            .expect_classify_and_save()
            .times(1)
            .return_once(move |_, _, _, _| {
                Box::pin(async move {
                    classify_gate.wait().await;
                    Ok(Some(Regex::new(r"/item/").unwrap()))
                })
            });
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let (sender, stop) = watch::channel(false);
        let result = stop_at_gate(
            service.run_until(
                &ListingSourceId::new(),
                &CrawlerDomainId::new(),
                ROOT,
                threshold,
                stop,
            ),
            sender,
            &gate,
        )
        .await;
        assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
    }

    #[rstest]
    #[case::streaming_batch(1)]
    #[case::final_batch(100)]
    #[tokio::test]
    async fn should_cancel_url_storage_without_starting_more_work(#[case] batch_size: usize) {
        let gate = Gate::default();
        let storage_gate = gate.clone();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        if batch_size == 100 {
            load_and_refresh_known_pattern(&mut patterns);
        } else {
            load_known_pattern(&mut patterns);
        }
        let mut urls = MockUrlMetadataRepository::new();
        urls.expect_upsert_links_batch().times(1).return_once(
            move |source, domain, urls, classes| {
                let records = stored_urls(source, domain, urls, classes);
                Box::pin(async move {
                    storage_gate.wait().await;
                    Ok(records)
                })
            },
        );
        let service = mock_service(spider, patterns, urls, batch_size);
        let (sender, stop) = watch::channel(false);
        let result = stop_at_gate(
            service.run_until(
                &ListingSourceId::new(),
                &CrawlerDomainId::new(),
                ROOT,
                20,
                stop,
            ),
            sender,
            &gate,
        )
        .await;
        assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
    }

    #[rstest]
    #[case::load(false)]
    #[case::threshold(true)]
    #[tokio::test(flavor = "current_thread")]
    async fn should_join_wrappers_before_returning_early_pattern_failure(
        #[case] at_threshold: bool,
    ) {
        let gate = Gate::default();
        let failure_gate = gate.clone();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, (1..=100).map(|i| format!("/item/{i}")));
        let mut patterns = MockUrlPatternService::new();
        if at_threshold {
            patterns
                .expect_load_pattern_for_domain()
                .returning(|_, _| Box::pin(async { Ok(None) }));
            patterns
                .expect_classify_and_save()
                .times(1)
                .return_once(move |_, _, _, _| {
                    Box::pin(async move {
                        failure_gate.wait().await;
                        Err(UrlPatternServiceError::Repository(sqlx::Error::RowNotFound))
                    })
                });
        } else {
            patterns
                .expect_load_pattern_for_domain()
                .times(1)
                .return_once(move |_, _| {
                    Box::pin(async move {
                        failure_gate.wait().await;
                        Err(UrlPatternServiceError::Repository(sqlx::Error::RowNotFound))
                    })
                });
        }
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let result = finish_failure_after_join_wait(
            service.run(&ListingSourceId::new(), &CrawlerDomainId::new(), ROOT, 20),
            &gate,
        )
        .await;
        assert!(matches!(
            result,
            Err(SpiderServiceError::UrlPattern(
                UrlPatternServiceError::Repository(sqlx::Error::RowNotFound)
            ))
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn should_join_wrappers_before_returning_early_storage_failure() {
        let gate = Gate::default();
        let storage_gate = gate.clone();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, (1..=100).map(|i| format!("/item/{i}")));
        let mut patterns = MockUrlPatternService::new();
        load_known_pattern(&mut patterns);
        let mut urls = MockUrlMetadataRepository::new();
        urls.expect_upsert_links_batch()
            .times(1)
            .return_once(move |_, _, _, _| {
                Box::pin(async move {
                    storage_gate.wait().await;
                    Err(UrlMetadataRepositoryError::Database {
                        source: sqlx::Error::RowNotFound,
                    })
                })
            });
        let service = mock_service(spider, patterns, urls, 1);
        let result = finish_failure_after_join_wait(
            service.run(&ListingSourceId::new(), &CrawlerDomainId::new(), ROOT, 20),
            &gate,
        )
        .await;
        assert!(
            matches!(result, Err(SpiderServiceError::UrlMetadata(error)) if matches!(*error, UrlMetadataRepositoryError::Database { source: sqlx::Error::RowNotFound }))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn should_join_wrappers_before_resuming_service_panic() {
        let gate = Gate::default();
        let panic_gate = gate.clone();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, (1..=100).map(|i| format!("/item/{i}")));
        let mut patterns = MockUrlPatternService::new();
        patterns
            .expect_load_pattern_for_domain()
            .times(1)
            .return_once(move |_, _| {
                Box::pin(async move {
                    panic_gate.wait().await;
                    std::panic::panic_any(42_u8)
                })
            });
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let result = finish_failure_after_join_wait(
            AssertUnwindSafe(service.run(
                &ListingSourceId::new(),
                &CrawlerDomainId::new(),
                ROOT,
                20,
            ))
            .catch_unwind(),
            &gate,
        )
        .await;
        assert_eq!(result.unwrap_err().downcast_ref::<u8>(), Some(&42));
    }

    #[tokio::test]
    async fn should_reject_missing_diagnostics_after_multiple_pages_without_checkpoint() {
        let mut spider = MockSpider::new();
        spider
            .expect_crawl()
            .times(1)
            .returning(|_| Box::pin(async { Ok(SpiderCrawl::fixture(fixture_pages(3), None)) }));
        let mut patterns = MockUrlPatternService::new();
        patterns
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let result =
            bounded(service.run(&ListingSourceId::new(), &CrawlerDomainId::new(), ROOT, 20)).await;
        assert!(matches!(
            result,
            Err(SpiderServiceError::Discovery(
                SpiderDiscoveryError::Incomplete(CrawlIncompleteError::MissingDiagnostics)
            ))
        ));
    }

    #[rstest]
    #[case::cancelled(true)]
    #[case::operation_error(false)]
    #[tokio::test]
    async fn should_not_hide_earlier_discovery_failure_during_cleanup(#[case] cancel: bool) {
        let (sender, stop) = watch::channel(false);
        let root_sender = sender.clone();
        let mut spider = MockSpider::new();
        spider.expect_crawl().times(1).return_once(move |_| {
            Box::pin(async move {
                let mut crawl = SpiderCrawl::fixture(fixture_pages(3), None);
                assert!(matches!(
                    crawl.completion().await,
                    Err(SpiderDiscoveryError::Incomplete(
                        CrawlIncompleteError::MissingDiagnostics
                    ))
                ));
                if cancel {
                    root_sender.send(true).unwrap();
                }
                Ok(crawl)
            })
        });
        let mut patterns = MockUrlPatternService::new();
        if !cancel {
            patterns
                .expect_load_pattern_for_domain()
                .times(1)
                .returning(|_, _| {
                    Box::pin(async {
                        Err(UrlPatternServiceError::Repository(sqlx::Error::RowNotFound))
                    })
                });
        }
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let result = bounded(service.run_until(
            &ListingSourceId::new(),
            &CrawlerDomainId::new(),
            ROOT,
            20,
            stop,
        ))
        .await;
        if cancel {
            assert!(matches!(
                result,
                Err(SpiderServiceError::Discovery(
                    SpiderDiscoveryError::Incomplete(CrawlIncompleteError::MissingDiagnostics)
                ))
            ));
        } else {
            assert!(matches!(
                result,
                Err(SpiderServiceError::UrlPattern(
                    UrlPatternServiceError::Repository(sqlx::Error::RowNotFound)
                ))
            ));
        }
    }

    #[rstest]
    #[case::missing(0)]
    #[case::short(19)]
    #[case::extra(21)]
    #[tokio::test]
    async fn should_reject_url_upsert_count_mismatch_without_checkpoint(#[case] actual: usize) {
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        load_and_refresh_known_pattern(&mut patterns);
        let mut urls = MockUrlMetadataRepository::new();
        urls.expect_upsert_links_batch().times(1).returning(
            move |source, domain, urls, classes| {
                let mut records = stored_urls(source, domain, urls, classes);
                records.resize(actual, records[0].clone());
                Box::pin(async move { Ok(records) })
            },
        );
        let service = mock_service(spider, patterns, urls, 100);
        let result =
            bounded(service.run(&ListingSourceId::new(), &CrawlerDomainId::new(), ROOT, 20)).await;
        assert!(
            matches!(result, Err(SpiderServiceError::UrlUpsertCountMismatch { expected: 20, actual: count }) if count == actual)
        );
    }

    #[tokio::test]
    async fn should_propagate_required_checkpoint_failure() {
        let source = ListingSourceId::new();
        let domain = CrawlerDomainId::new();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        load_and_refresh_known_pattern(&mut patterns);
        patterns
            .expect_mark_as_crawled()
            .times(1)
            .returning(|_, _| {
                Box::pin(async {
                    Err(UrlPatternServiceError::Repository(sqlx::Error::RowNotFound))
                })
            });
        let mut urls = MockUrlMetadataRepository::new();
        setup_mock_url_repo(&mut urls, 1, domain);
        let service = mock_service(spider, patterns, urls, 100);
        let result = bounded(service.run(&source, &domain, ROOT, 20)).await;
        assert!(matches!(
            result,
            Err(SpiderServiceError::UrlPattern(
                UrlPatternServiceError::Repository(sqlx::Error::RowNotFound)
            ))
        ));
    }

    #[rstest]
    #[case::stop_committed(false, false)]
    #[case::stop_failed(false, true)]
    #[case::sender_lost_committed(true, false)]
    #[case::sender_lost_failed(true, true)]
    #[tokio::test]
    async fn should_await_started_checkpoint_and_report_actual_result(
        #[case] lose_sender: bool,
        #[case] checkpoint_fails: bool,
    ) {
        let source = ListingSourceId::new();
        let domain = CrawlerDomainId::new();
        let gate = Gate::default();
        let checkpoint_gate = gate.clone();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        load_and_refresh_known_pattern(&mut patterns);
        patterns
            .expect_mark_as_crawled()
            .times(1)
            .return_once(move |_, _| {
                Box::pin(async move {
                    checkpoint_gate.wait().await;
                    if checkpoint_fails {
                        Err(UrlPatternServiceError::Repository(sqlx::Error::RowNotFound))
                    } else {
                        Ok(())
                    }
                })
            });
        let mut urls = MockUrlMetadataRepository::new();
        setup_mock_url_repo(&mut urls, 1, domain);
        let service = mock_service(spider, patterns, urls, 100);
        let (sender, stop) = watch::channel(false);
        let work = service.run_until(&source, &domain, ROOT, 20, stop);
        tokio::pin!(work);
        let result = bounded(async {
            tokio::select! {
                _ = &mut work => panic!("service finished before checkpoint started"),
                _ = gate.started.notified() => {}
            }
            if lose_sender {
                drop(sender);
            } else {
                sender.send(true).unwrap();
            }
            assert!(
                futures::poll!(&mut work).is_pending(),
                "started checkpoint must not be cancelled"
            );
            assert!(!gate.dropped.load(Ordering::SeqCst));
            gate.release.notify_one();
            work.await
        })
        .await;
        assert!(gate.dropped.load(Ordering::SeqCst));
        if checkpoint_fails {
            assert!(matches!(
                result,
                Err(SpiderServiceError::UrlPattern(
                    UrlPatternServiceError::Repository(sqlx::Error::RowNotFound)
                ))
            ));
        } else {
            let result = result.unwrap();
            assert_eq!(result.total_links, 20);
            assert_eq!(result.product_urls_count, 20);
        }
    }

    #[rstest]
    #[case::full_write(20)]
    #[case::short_write(19)]
    #[tokio::test]
    async fn should_not_start_checkpoint_when_final_url_write_returns_after_stop(
        #[case] actual: usize,
    ) {
        let (sender, stop) = watch::channel(false);
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        load_and_refresh_known_pattern(&mut patterns);
        let mut urls = MockUrlMetadataRepository::new();
        urls.expect_upsert_links_batch().times(1).return_once(
            move |source, domain, urls, classes| {
                let mut records = stored_urls(source, domain, urls, classes);
                records.truncate(actual);
                Box::pin(async move {
                    sender.send(true).unwrap();
                    Ok(records)
                })
            },
        );
        let service = mock_service(spider, patterns, urls, 100);
        let result = bounded(service.run_until(
            &ListingSourceId::new(),
            &CrawlerDomainId::new(),
            ROOT,
            20,
            stop,
        ))
        .await;
        if actual == 20 {
            assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
        } else {
            assert!(matches!(
                result,
                Err(SpiderServiceError::UrlUpsertCountMismatch {
                    expected: 20,
                    actual: 19
                })
            ));
        }
    }

    #[tokio::test]
    async fn should_not_start_url_storage_when_classification_returns_after_stop() {
        let (sender, stop) = watch::channel(false);
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        patterns
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        patterns
            .expect_classify_and_save()
            .times(1)
            .return_once(move |_, _, _, _| {
                Box::pin(async move {
                    sender.send(true).unwrap();
                    Ok(Some(Regex::new(r"/item/").unwrap()))
                })
            });
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let result = bounded(service.run_until(
            &ListingSourceId::new(),
            &CrawlerDomainId::new(),
            ROOT,
            20,
            stop,
        ))
        .await;
        assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
    }

    #[tokio::test]
    async fn should_not_start_classification_when_pattern_load_returns_after_stop() {
        let (sender, stop) = watch::channel(false);
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        patterns
            .expect_load_pattern_for_domain()
            .times(1)
            .return_once(move |_, _| {
                Box::pin(async move {
                    sender.send(true).unwrap();
                    Ok(None)
                })
            });
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let result = bounded(service.run_until(
            &ListingSourceId::new(),
            &CrawlerDomainId::new(),
            ROOT,
            20,
            stop,
        ))
        .await;
        assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
    }

    #[tokio::test]
    async fn should_keep_partial_progress_retryable_without_claiming_exactly_once() {
        let source = ListingSourceId::new();
        let domain = CrawlerDomainId::new();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, (1..=4).map(|i| format!("/item/{i}")));
        let mut patterns = MockUrlPatternService::new();
        load_known_pattern(&mut patterns);
        let checkpoints = Arc::new(AtomicUsize::new(0));
        let marked = checkpoints.clone();
        patterns
            .expect_mark_as_crawled()
            .times(1)
            .returning(move |_, _| {
                marked.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            });
        let persisted = Arc::new(Mutex::new(BTreeSet::new()));
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let persisted_urls = persisted.clone();
        let url_attempts = attempts.clone();
        let mut urls = MockUrlMetadataRepository::new();
        urls.expect_upsert_links_batch().times(4).returning(
            move |source, domain, urls, classes| {
                let records = stored_urls(source, domain, urls, classes);
                let batch: Vec<String> = urls.iter().map(ToString::to_string).collect();
                let attempt = {
                    let mut attempts = url_attempts.lock().unwrap();
                    attempts.push(batch.clone());
                    attempts.len()
                };
                let persisted_urls = persisted_urls.clone();
                Box::pin(async move {
                    if attempt == 2 {
                        return Err(UrlMetadataRepositoryError::Database {
                            source: sqlx::Error::RowNotFound,
                        });
                    }
                    persisted_urls.lock().unwrap().extend(batch);
                    Ok(records)
                })
            },
        );
        let service = mock_service(spider, patterns, urls, 2);
        let first = bounded(service.run(&source, &domain, ROOT, 20)).await;
        assert!(matches!(first, Err(SpiderServiceError::UrlMetadata(_))));
        assert_eq!(checkpoints.load(Ordering::SeqCst), 0);
        assert_eq!(persisted.lock().unwrap().len(), 2);

        let retry = bounded(service.run(&source, &domain, ROOT, 20))
            .await
            .unwrap();
        assert_eq!(retry.total_links, 4);
        assert_eq!(retry.product_urls_count, 4);
        assert_eq!(checkpoints.load(Ordering::SeqCst), 1);
        assert_eq!(persisted.lock().unwrap().len(), 4);
        let attempts = attempts.lock().unwrap();
        assert_eq!(attempts.len(), 4);
        assert_eq!(
            attempts[0], attempts[2],
            "retry repeats the previously persisted batch"
        );
    }

    #[tokio::test]
    async fn should_leave_url_buffer_retryable_when_processing_is_stopped() {
        let service = mock_service(
            MockSpider::new(),
            MockUrlPatternService::new(),
            MockUrlMetadataRepository::new(),
            100,
        );
        let (_sender, stop) = watch::channel(true);
        let mut pages = fixture_pages(3);
        let result = service
            .process_buffer(
                &mut pages,
                &ListingSourceId::new(),
                &CrawlerDomainId::new(),
                &ProductListingPattern::Known(Regex::new(r"/item/").unwrap()),
                &stop,
            )
            .await;
        assert!(matches!(result, Err(SpiderServiceError::Cancelled)));
        assert_eq!(pages.len(), 3);
    }

    #[tokio::test]
    async fn should_preserve_pending_review_error_without_urls_or_checkpoint() {
        let source = ListingSourceId::new();
        let domain = CrawlerDomainId::new();
        let review = crate::CrawlerReviewId::new();
        let mut spider = MockSpider::new();
        setup_mock_crawl(&mut spider, ROOT, item_pages());
        let mut patterns = MockUrlPatternService::new();
        patterns
            .expect_load_pattern_for_domain()
            .returning(|_, _| Box::pin(async { Ok(None) }));
        patterns
            .expect_classify_and_save()
            .times(1)
            .returning(move |_, _, _, _| {
                Box::pin(async move {
                    Err(UrlPatternServiceError::PendingReview {
                        listing_source_id: source,
                        review_id: review,
                    })
                })
            });
        let service = mock_service(spider, patterns, MockUrlMetadataRepository::new(), 100);
        let result = bounded(service.run(&source, &domain, ROOT, 20)).await;
        assert!(
            matches!(result, Err(SpiderServiceError::UrlPattern(UrlPatternServiceError::PendingReview { listing_source_id, review_id })) if listing_source_id == source && review_id == review)
        );
    }

    #[tokio::test]
    async fn should_keep_outside_domain_pages_out_of_url_writes_and_completion_counts() {
        let source = ListingSourceId::new();
        let domain = CrawlerDomainId::new();
        let mut spider = MockSpider::new();
        spider.expect_crawl().times(1).returning(|_| {
            Box::pin(async {
                let mut pages = fixture_pages(2);
                pages.insert(
                    0,
                    CrawledPage {
                        url: CrawledUrl::new(Url::parse("https://outside.example/item/1").unwrap()),
                    },
                );
                Ok(SpiderCrawl::fixture(
                    pages,
                    Some(CrawlDiagnostics::default()),
                ))
            })
        });
        let mut patterns = MockUrlPatternService::new();
        load_known_pattern(&mut patterns);
        setup_mock_mark_as_crawled(&mut patterns, ROOT);
        let mut urls = MockUrlMetadataRepository::new();
        urls.expect_upsert_links_batch()
            .times(2)
            .withf(move |listing_source_id, domain_id, urls, _| {
                *listing_source_id == source
                    && *domain_id == domain
                    && urls.iter().all(|url| url.host_str() == Some("example.com"))
            })
            .returning(|source, domain, urls, classes| {
                let records = stored_urls(source, domain, urls, classes);
                Box::pin(async move { Ok(records) })
            });
        let service = mock_service(spider, patterns, urls, 1);
        let result = bounded(service.run(&source, &domain, ROOT, 20))
            .await
            .unwrap();
        assert_eq!(result.total_links, 2);
        assert_eq!(result.product_urls_count, 2);
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

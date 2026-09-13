use crate::scraper::scraper_service::{
    DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE, DEFAULT_SCHEMA_SEED_PAGES, ScraperAutoThrottleConfig,
};
use crate::spider::discovery::website_spider::CrawlerConfig as SpiderCrawlerConfig;
use std::time::Duration;

#[derive(Clone)]
pub struct CrawlerCronConfig {
    pub spider_interval: Duration,
    pub scraper_interval: Duration,
    pub listing_source_sync_interval: Duration,
    /// Optional number of domains fetched per scraper scheduler refill.
    /// Defaults to scraper concurrency.
    pub scraper_domain_batch_size: Option<usize>,
    /// Maximum URLs fetched per selected scraper domain.
    pub scraper_urls_per_domain: i64,
    /// Number of scraped products to accumulate before flush.
    pub push_batch_size: usize,
    /// Maximum product messages buffered between scraper workers and the
    /// single product-push collector.
    pub push_queue_capacity: usize,
    /// Maximum age of the oldest item in a partial product-push batch.
    pub push_max_batch_age: Duration,
    /// Maximum number of unique ProductListing upsert transactions executed in parallel
    /// by one product-push batch.
    pub push_max_concurrency: usize,
    /// Maximum connections in the authoritative business Postgres pool used by
    /// canonical ListingSource reads and ProductListing writes.
    pub business_db_max_connections: u32,
    pub spider_concurrency: usize,
    /// Per-site in-flight crawl limit for spider::Website.
    pub spider_site_concurrency_limit: usize,
    pub scraper_concurrency: usize,
    pub spider_classify_threshold: usize,
    /// Number of pages used to seed first-time schema generation per ListingSource.
    /// `1` means current page only; higher values fetch additional random
    /// product pages on schema cache miss.
    pub scraper_schema_seed_pages: usize,
    /// Minimum delay before scraper requests for the same domain.
    pub scraper_domain_delay: Duration,
    /// Desired average in-flight scraper requests per domain for adaptive pacing.
    pub scraper_auto_throttle_target_concurrency: f64,
    /// Maximum adaptive delay before scraper requests for a slow domain.
    pub scraper_auto_throttle_max_delay: Duration,
    /// Smoothing factor for per-domain scraper fetch latency.
    pub scraper_auto_throttle_alpha: f64,
    /// Hard per-ListingSource budget for schema-generation LLM calls.
    pub scraper_max_llm_calls_per_listing_source: i64,
    /// Maximum Postgres connections for crawler queries.
    pub db_max_connections: Option<u32>,
}

impl Default for CrawlerCronConfig {
    fn default() -> Self {
        Self {
            spider_interval: Duration::from_secs(600), // 10 minutes
            scraper_interval: Duration::from_secs(60), // 1 minute
            listing_source_sync_interval: Duration::from_secs(10800), // 3 hours
            scraper_domain_batch_size: None,
            scraper_urls_per_domain: 100,
            push_batch_size: 25,
            push_queue_capacity: 100,
            push_max_batch_age: Duration::from_secs(5),
            push_max_concurrency: 4,
            business_db_max_connections: 8,
            spider_concurrency: 3,
            spider_site_concurrency_limit: 8,
            scraper_concurrency: 10,
            spider_classify_threshold: 200,
            scraper_schema_seed_pages: DEFAULT_SCHEMA_SEED_PAGES,
            scraper_domain_delay: Duration::from_secs(1),
            scraper_auto_throttle_target_concurrency: 2.0,
            scraper_auto_throttle_max_delay: Duration::from_secs(10),
            scraper_auto_throttle_alpha: 0.15,
            scraper_max_llm_calls_per_listing_source: DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
            db_max_connections: None,
        }
    }
}

impl CrawlerCronConfig {
    pub fn effective_push_batch_size(&self) -> usize {
        self.push_batch_size.max(1)
    }

    pub fn effective_push_queue_capacity(&self) -> usize {
        self.push_queue_capacity.max(1)
    }

    pub fn effective_push_max_batch_age(&self) -> Duration {
        self.push_max_batch_age.max(Duration::from_millis(1))
    }

    pub fn effective_push_max_concurrency(&self) -> usize {
        self.push_max_concurrency.max(1)
    }

    pub fn effective_business_db_max_connections(&self) -> u32 {
        self.business_db_max_connections.max(3)
    }

    pub fn validate_business_capacity(&self) {
        let push = self.effective_push_max_concurrency() as u32;
        let pool = self.effective_business_db_max_connections();

        assert!(
            push + 2 <= pool,
            "product push concurrency must leave business database headroom"
        );
    }

    pub fn effective_db_max_connections(&self) -> u32 {
        self.db_max_connections
            .unwrap_or_else(|| (self.spider_concurrency + self.scraper_concurrency + 10) as u32)
    }

    pub fn spider_website_config(&self) -> SpiderCrawlerConfig {
        SpiderCrawlerConfig {
            concurrency_limit: self.spider_site_concurrency_limit,
            ..SpiderCrawlerConfig::default()
        }
    }

    pub fn scraper_auto_throttle_config(&self) -> ScraperAutoThrottleConfig {
        ScraperAutoThrottleConfig {
            target_concurrency: self.scraper_auto_throttle_target_concurrency,
            min_delay: self.scraper_domain_delay,
            max_delay: self.scraper_auto_throttle_max_delay,
            alpha: self.scraper_auto_throttle_alpha,
            enabled: true,
        }
    }

    pub fn effective_scraper_domain_batch_size(&self) -> usize {
        self.scraper_domain_batch_size
            .unwrap_or(self.scraper_concurrency)
            .max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_clamp_push_limits_to_non_zero_values() {
        let config = CrawlerCronConfig {
            push_batch_size: 0,
            push_queue_capacity: 0,
            push_max_batch_age: Duration::ZERO,
            push_max_concurrency: 0,
            business_db_max_connections: 0,
            ..CrawlerCronConfig::default()
        };

        assert_eq!(config.effective_push_batch_size(), 1);
        assert_eq!(config.effective_push_queue_capacity(), 1);
        assert_eq!(
            config.effective_push_max_batch_age(),
            Duration::from_millis(1)
        );
        assert_eq!(config.effective_push_max_concurrency(), 1);
        assert!(config.effective_business_db_max_connections() >= 3);
    }

    #[test]
    #[should_panic]
    fn should_reject_business_pool_without_push_headroom() {
        let config = CrawlerCronConfig {
            push_max_concurrency: 8,
            business_db_max_connections: 8,
            ..Default::default()
        };

        config.validate_business_capacity();
    }

    #[test]
    fn should_accept_business_pool_with_push_headroom() {
        let config = CrawlerCronConfig {
            push_max_concurrency: 4,
            business_db_max_connections: 8,
            ..Default::default()
        };

        config.validate_business_capacity();
    }
}

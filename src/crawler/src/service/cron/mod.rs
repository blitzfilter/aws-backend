mod config;
mod job;
mod metrics;
mod scraper;
mod spider;

pub use config::CrawlerCronConfig;
pub use job::CrawlerCronJob;

#[cfg(test)]
pub(super) mod test_support {
    use crate::scraper::candidate_service::ScraperCandidate;
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, MockListingSourceRegistrationRepository,
        MockListingSourceRegistrationSource,
    };
    use crate::service::raw_capture::MockProductListingRawCaptureService;
    use listing_source_core::ListingSourceId;

    pub(super) fn noop_listing_source_registration() -> ListingSourceRegistrationService {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_apply_snapshot()
            .returning(|_| {
                Box::pin(async {
                    Ok(crate::service::listing_source_registration::ListingSourceSnapshotResult::default())
                })
            });
        ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
    }

    pub(super) fn noop_raw_capture() -> Box<MockProductListingRawCaptureService> {
        let mut capture = MockProductListingRawCaptureService::new();
        capture
            .expect_capture()
            .returning(|observations| Box::pin(async move { vec![true; observations.len()] }));
        Box::new(capture)
    }

    pub(super) fn scraper_candidate(listing_source_name: &str, url: url::Url) -> ScraperCandidate {
        ScraperCandidate {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: listing_source_name.to_string(),
            url_pattern: None,
            url,
            last_scraped_hash: None,
            last_scraped_schema_fingerprint: None,
            last_captured_raw_input_sha256: None,
        }
    }
}

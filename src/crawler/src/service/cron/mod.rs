mod config;
mod job;
mod metrics;
mod scraper;
mod spider;

pub use config::CrawlerCronConfig;
pub use job::{CrawlerCronJob, CrawlerRunError};

#[cfg(test)]
pub(super) mod test_support {
    use crate::scraper::candidate_service::ScraperCandidate;
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, MockListingSourceRegistrationRepository,
        MockListingSourceRegistrationSource,
    };
    use crate::service::raw_capture::{
        MockProductListingRawCaptureService, ProductListingRawCaptureOutcome,
    };
    use listing_source_core::ListingSourceId;

    #[derive(Clone, Default)]
    pub(super) struct PendingWork {
        pub(super) entered: std::sync::Arc<tokio::sync::Notify>,
        pub(super) dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    struct DropFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    impl PendingWork {
        pub(super) async fn wait<T>(&self) -> T {
            let _guard = DropFlag(self.dropped.clone());
            self.entered.notify_one();
            std::future::pending().await
        }
    }

    pub(super) async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(2), future)
            .await
            .expect("fake work must stop and join")
    }

    pub(super) struct PanicOnDrop;

    impl Drop for PanicOnDrop {
        fn drop(&mut self) {
            panic!("injected future destructor failure");
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub(super) enum RetryableScraperFailure {
        RateLimit,
        Budget,
        Review,
        Provider,
    }

    impl RetryableScraperFailure {
        pub(super) fn error(
            self,
            candidate: &ScraperCandidate,
        ) -> crate::scraper::scraper_service::ScraperError {
            use crate::scraper::scraper_service::ScraperError;
            match self {
                Self::RateLimit => ScraperError::HttpError {
                    url: candidate.url.clone(),
                    kind: crate::network::policy::NetworkErrorKind::HttpStatus(429),
                    details: "fake rate limit".into(),
                },
                Self::Budget => ScraperError::LlmBudgetExceeded {
                    listing_source_id: candidate.listing_source_id,
                    url: candidate.url.clone(),
                    max_calls: 10,
                },
                Self::Review => ScraperError::PendingSchemaReview {
                    url: candidate.url.clone(),
                    review_id: crate::CrawlerReviewId::new(),
                },
                Self::Provider => ScraperError::SchemaServiceError(
                    crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError::LargeLanguageModelError(
                        large_language_model::LargeLanguageModelError::Timeout {
                            source: application::error::static_error("fake provider timeout"),
                        },
                    ),
                ),
            }
        }

        pub(super) fn expect_metadata(
            self,
            service: &mut crate::scraper::candidate_service::MockScraperCandidateService,
            candidate: &ScraperCandidate,
            fails: bool,
        ) {
            use crate::spider::classification::url_metadata::CrawlerUrlWriteOutcome;
            let listing_source_id = candidate.listing_source_id;
            let candidate_url = candidate.url.clone();
            let expected_hash = candidate.last_captured_raw_input_sha256.clone();
            let result = move || {
                if fails {
                    Err(sqlx::Error::PoolClosed)
                } else {
                    Ok(CrawlerUrlWriteOutcome::Applied)
                }
            };
            if matches!(self, Self::Provider) {
                service
                    .expect_mark_scraper_failure()
                    .once()
                    .withf(move |id, url, kind, _, expected| {
                        *id == listing_source_id
                            && *url == candidate_url
                            && kind == "SchemaServiceError"
                            && *expected == expected_hash.as_deref()
                    })
                    .returning(move |_, _, _, _, _| Box::pin(std::future::ready(result())));
            } else {
                let (kind, status, cooldown) = match self {
                    Self::RateLimit => ("HttpStatus(429)", Some(429), 10),
                    Self::Budget => ("LlmBudgetExceeded", None, 1800),
                    Self::Review => ("PendingSchemaReview", None, 1800),
                    Self::Provider => unreachable!(),
                };
                service
                    .expect_mark_fetch_failure()
                    .once()
                    .withf(
                        move |id, url, actual_kind, _, actual_status, next, expected| {
                            let seconds = (*next - time::OffsetDateTime::now_utc()).whole_seconds();
                            *id == listing_source_id
                                && *url == candidate_url
                                && actual_kind == kind
                                && *actual_status == status
                                && (cooldown - 2..=cooldown + 2).contains(&seconds)
                                && *expected == expected_hash.as_deref()
                        },
                    )
                    .returning(move |_, _, _, _, _, _, _| Box::pin(std::future::ready(result())));
            }
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub(super) enum GeneratedPatternFailure {
        Provider,
        Regex,
        NoProducts,
    }

    impl GeneratedPatternFailure {
        pub(super) fn error(self) -> crate::spider::service::SpiderServiceError {
            use crate::spider::classification::url_classification_service::UrlClassificationError;
            use crate::spider::classification::url_pattern_service::UrlPatternServiceError;
            crate::spider::service::SpiderServiceError::UrlPattern(
                UrlPatternServiceError::Classification(match self {
                    Self::Provider => UrlClassificationError::Llm("fake provider timeout".into()),
                    Self::Regex => UrlClassificationError::Regex(regex::Error::Syntax(
                        "fake generated pattern".into(),
                    )),
                    Self::NoProducts => {
                        UrlClassificationError::NoProducts("fake empty generation".into())
                    }
                }),
            )
        }
    }

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
        capture.expect_capture().returning(|observations| {
            Box::pin(
                async move { vec![ProductListingRawCaptureOutcome::Persisted; observations.len()] },
            )
        });
        Box::new(capture)
    }

    pub(super) fn scraper_candidate(listing_source_name: &str, url: url::Url) -> ScraperCandidate {
        ScraperCandidate {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: listing_source_name.to_string(),
            fallback_currency: None,
            url_pattern: None,
            url,
            last_scraped_hash: None,
            last_scraped_schema_fingerprint: None,
            last_captured_raw_input_sha256: None,
        }
    }
}

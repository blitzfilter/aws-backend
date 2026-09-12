use super::*;
use crate::scraper::raw_input::crawler_verified_removal_input;
use crate::scraper::scraper_service::ScrapedProduct;
use crate::service::cron::test_support::{
    GeneratedPatternFailure, PanicOnDrop, PendingWork, RetryableScraperFailure, bounded,
    noop_listing_source_registration, scraper_candidate,
};
use crate::service::listing_source_registration::{
    ListingSourceSnapshotResult, ListingSourceSyncError,
};
use crate::service::raw_capture::{
    MockProductListingRawCaptureService, ProductListingRawCaptureOutcome,
};
use crate::spider::classification::url_metadata::CrawlerUrlWriteOutcome;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

fn job() -> CrawlerCronJob {
    CrawlerCronJob::new(
        CrawlerCronConfig {
            spider_concurrency: 0,
            scraper_concurrency: 0,
            ..Default::default()
        },
        Arc::new(LocalLockManager::new()),
        Box::new(MockSpiderCandidateService::new()),
        Box::new(MockSpiderService::new()),
        Box::new(MockScraperCandidateService::new()),
        Box::new(MockScraperService::new()),
        noop_listing_source_registration(),
        noop_raw_capture(),
    )
}

fn registration(
    source: MockListingSourceRegistrationSource,
    repository: MockListingSourceRegistrationRepository,
) -> Arc<ListingSourceRegistrationService> {
    Arc::new(ListingSourceRegistrationService::new(
        Box::new(source),
        Box::new(repository),
    ))
}

fn snapshot_repository() -> MockListingSourceRegistrationRepository {
    let mut repository = MockListingSourceRegistrationRepository::new();
    repository
        .expect_apply_snapshot()
        .returning(|_| Box::pin(async { Ok(ListingSourceSnapshotResult::default()) }));
    repository
}

#[rstest::rstest]
#[case::rate_limit(RetryableScraperFailure::RateLimit)]
#[case::budget(RetryableScraperFailure::Budget)]
#[case::review(RetryableScraperFailure::Review)]
#[case::provider(RetryableScraperFailure::Provider)]
#[tokio::test]
async fn should_keep_daemon_alive_for_next_pass_after_retryable_scraper_error(
    #[case] failure: RetryableScraperFailure,
) {
    let selected = scraper_candidate("retry", "https://example.test/retry".parse().unwrap());
    let error = failure.error(&selected);
    let next_pass = PendingWork::default();
    let pending = next_pass.clone();
    let selections = Arc::new(AtomicUsize::new(0));
    let calls = selections.clone();
    let mut candidates = MockScraperCandidateService::new();
    failure.expect_metadata(&mut candidates, &selected, false);
    candidates
        .expect_get_candidates()
        .once()
        .return_once(move |_, _, excluded| {
            assert!(excluded.is_empty());
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(vec![selected]) })
        });
    let calls = selections.clone();
    candidates
        .expect_get_candidates()
        .once()
        .returning(move |_, _, excluded| {
            assert!(!excluded.is_empty());
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(vec![]) })
        });
    let calls = selections.clone();
    candidates
        .expect_get_candidates()
        .once()
        .return_once(move |_, _, excluded| {
            assert!(excluded.is_empty());
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { pending.wait().await })
        });
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .once()
        .return_once(move |_, _, _, _, _, _| Box::pin(async move { Err(error) }));
    let mut job = job();
    job.config.scraper_concurrency = 1;
    job.config.scraper_interval = Duration::ZERO;
    job.scraper_candidates = Arc::new(candidates);
    job.scraper_service = Arc::new(scraper);
    let (stop_tx, stop) = watch::channel(false);
    let (failure_tx, failed) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(job.run_until_with_failure(stop, failure_tx), async {
            next_pass.entered.notified().await;
            assert!(
                !*failed.borrow(),
                "expected retry must not start runtime failure drain"
            );
            stop_tx.send_replace(true);
        })
    })
    .await;
    assert_eq!(result, Ok(()));
    assert!(!*failed.borrow());
    assert!(next_pass.dropped.load(Ordering::SeqCst));
    assert_eq!(selections.load(Ordering::SeqCst), 3);
}

#[rstest::rstest]
#[case::provider(GeneratedPatternFailure::Provider)]
#[case::generated_regex(GeneratedPatternFailure::Regex)]
#[case::no_products(GeneratedPatternFailure::NoProducts)]
#[tokio::test]
async fn should_keep_daemon_alive_after_generated_pattern_error(
    #[case] failure: GeneratedPatternFailure,
) {
    let next_pass = PendingWork::default();
    let pending = next_pass.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let selected = calls.clone();
    let mut candidates = MockSpiderCandidateService::new();
    candidates
        .expect_get_candidates()
        .times(3)
        .returning(move |_, excluded| {
            let call = selected.fetch_add(1, Ordering::SeqCst);
            let pending = pending.clone();
            assert_eq!(excluded.is_empty(), call != 1);
            Box::pin(async move {
                match call {
                    0 => Ok(vec![crate::spider::candidate_service::SpiderCandidate {
                        listing_source_id: ListingSourceId::new(),
                        domain_id: crate::CrawlerDomainId::new(),
                        listing_source_domain: "example.test".into(),
                        crawl_failure_count: 0,
                        last_crawl_error_kind: None,
                    }]),
                    1 => Ok(vec![]),
                    _ => pending.wait().await,
                }
            })
        });
    candidates
        .expect_mark_crawl_failure()
        .once()
        .returning(|_, kind, count, next| {
            assert_eq!(kind, "spider_run_error");
            assert_eq!(count, 1);
            let seconds = (next - time::OffsetDateTime::now_utc()).whole_seconds();
            assert!((298..=302).contains(&seconds));
            Box::pin(async { Ok(()) })
        });
    let mut service = MockSpiderService::new();
    service
        .expect_run_until()
        .once()
        .returning(move |_, _, _, _, _| Box::pin(async move { Err(failure.error()) }));
    let mut job = job();
    job.config.spider_concurrency = 1;
    job.config.spider_interval = Duration::ZERO;
    job.spider_candidates = Arc::new(candidates);
    job.spider_service = Arc::new(service);
    let (stop_tx, stop) = watch::channel(false);
    let (failure_tx, failed) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(job.run_until_with_failure(stop, failure_tx), async {
            next_pass.entered.notified().await;
            assert!(!*failed.borrow());
            stop_tx.send_replace(true);
        })
    })
    .await;
    assert_eq!(result, Ok(()));
    assert!(!*failed.borrow());
    assert!(next_pass.dropped.load(Ordering::SeqCst));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[rstest::rstest]
#[case::producer_poll_panic(false, false)]
#[case::producer_destructor_panic(true, false)]
#[case::producer_destructor_panic_during_stop(true, true)]
#[tokio::test]
async fn should_notify_spider_and_runtime_before_releasing_accepted_collector(
    #[case] drop_panic: bool,
    #[case] stop_first: bool,
) {
    let spider_entered = Arc::new(Notify::new());
    let spider_stopped = Arc::new(Notify::new());
    let spider_joined = Arc::new(AtomicBool::new(false));
    let mut spider = MockSpiderService::new();
    let (entered, stopped, joined) = (
        spider_entered.clone(),
        spider_stopped.clone(),
        spider_joined.clone(),
    );
    spider
        .expect_run_until()
        .once()
        .return_once(move |_, _, _, _, mut stop| {
            Box::pin(async move {
                entered.notify_one();
                wait_for_stop(&mut stop).await;
                assert!(*stop.borrow());
                joined.store(true, Ordering::SeqCst);
                stopped.notify_one();
                Err(crate::spider::service::SpiderServiceError::Cancelled)
            })
        });
    let spider_selections = Arc::new(AtomicUsize::new(0));
    let calls = spider_selections.clone();
    let mut spider_candidates = MockSpiderCandidateService::new();
    spider_candidates
        .expect_get_candidates()
        .once()
        .returning(move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok(vec![crate::spider::candidate_service::SpiderCandidate {
                    listing_source_id: ListingSourceId::new(),
                    domain_id: crate::CrawlerDomainId::new(),
                    listing_source_domain: "spider.test".into(),
                    crawl_failure_count: 0,
                    last_crawl_error_kind: None,
                }])
            })
        });
    let fetch = PendingWork::default();
    let trigger_entered = Arc::new(Notify::new());
    let trigger = Arc::new(Notify::new());
    let (pending, entered, fail) = (fetch.clone(), trigger_entered.clone(), trigger.clone());
    let mut scraper = MockScraperService::new();
    scraper.expect_scrape().times(3).returning(move |_, url, _, _, _, _| {
        let (pending, entered, fail) = (pending.clone(), entered.clone(), fail.clone());
        let path = url.path().to_owned();
        let raw_input = crawler_verified_removal_input(url).unwrap();
        Box::pin(async move {
            match path.as_str() {
                "/accepted" => Ok(Some(ScrapedProduct {
                    raw_input, availability: product_listing_normalization::ListingAvailabilityQuickCheck::NoAssertion,
                    hash: "page".into(), schema_fingerprint: "schema".into(), raw_input_sha256: vec![3; 32],
                })),
                "/blocked" => pending.wait().await,
                "/fail" => {
                    let _bomb = drop_panic.then(|| PanicOnDrop);
                    entered.notify_one();
                    if stop_first { return std::future::pending().await; }
                    fail.notified().await;
                    assert!(drop_panic, "injected producer poll failure");
                    Ok(None)
                }
                _ => panic!("must not admit another URL after failure"),
            }
        })
    });
    let scraper_selections = Arc::new(AtomicUsize::new(0));
    let calls = scraper_selections.clone();
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .once()
        .returning(move |_, _, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Ok([
                    "https://accepted.test/accepted",
                    "https://accepted.test/blocked",
                    "https://accepted.test/never",
                    "https://failed.test/fail",
                ]
                .map(|url| scraper_candidate("fake", url.parse().unwrap()))
                .into())
            })
        });
    let marked = Arc::new(AtomicBool::new(false));
    let mark = marked.clone();
    candidates
        .expect_mark_as_scraped()
        .once()
        .returning(move |_, url, _, _, _, _, _| {
            assert_eq!(url.path(), "/accepted");
            mark.store(true, Ordering::SeqCst);
            Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
        });
    let capture_entered = Arc::new(Notify::new());
    let capture_release = Arc::new(Notify::new());
    let capture_finished = Arc::new(AtomicBool::new(false));
    let (entered, release, finished) = (
        capture_entered.clone(),
        capture_release.clone(),
        capture_finished.clone(),
    );
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().return_once(move |items| {
        Box::pin(async move {
            assert_eq!(items.len(), 1);
            entered.notify_one();
            release.notified().await;
            finished.store(true, Ordering::SeqCst);
            vec![ProductListingRawCaptureOutcome::Persisted]
        })
    });
    let mut job = job();
    job.config.spider_concurrency = 1;
    job.config.scraper_concurrency = 2;
    job.config.push_batch_size = 1;
    job.spider_candidates = Arc::new(spider_candidates);
    job.spider_service = Arc::new(spider);
    job.scraper_candidates = Arc::new(candidates);
    job.scraper_service = Arc::new(scraper);
    job.raw_capture = Arc::new(capture);
    let returned = AtomicBool::new(false);
    let (stop_tx, stop) = watch::channel(false);
    let (failure_tx, mut failed) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(
            async {
                let result = job.run_until_with_failure(stop, failure_tx).await;
                returned.store(true, Ordering::SeqCst);
                result
            },
            async {
                spider_entered.notified().await;
                fetch.entered.notified().await;
                trigger_entered.notified().await;
                capture_entered.notified().await;
                if stop_first {
                    stop_tx.send_replace(true);
                } else {
                    trigger.notify_one();
                }
                wait_for_stop(&mut failed).await;
                assert!(
                    *failed.borrow(),
                    "runtime must see failure before collector release"
                );
                spider_stopped.notified().await;
                assert!(!returned.load(Ordering::SeqCst), "retain the whole pass");
                assert!(!capture_finished.load(Ordering::SeqCst));
                assert!(!marked.load(Ordering::SeqCst));
                assert_eq!(spider_selections.load(Ordering::SeqCst), 1);
                assert_eq!(scraper_selections.load(Ordering::SeqCst), 1);
                capture_release.notify_one();
            }
        )
    })
    .await;
    assert_eq!(result, Err(CrawlerRunError::ScraperPassIncomplete));
    assert!(capture_finished.load(Ordering::SeqCst));
    assert!(marked.load(Ordering::SeqCst));
    assert!(fetch.dropped.load(Ordering::SeqCst));
    assert!(spider_joined.load(Ordering::SeqCst));
    assert_eq!(spider_selections.load(Ordering::SeqCst), 1);
    assert_eq!(scraper_selections.load(Ordering::SeqCst), 1);
}

#[rstest::rstest]
#[case::worker_panic(true)]
#[case::completed_capture_failure(false)]
#[tokio::test]
async fn should_reject_failed_scraper_pass_and_join_spider_sibling(#[case] worker_panic: bool) {
    let spider_entered = Arc::new(Notify::new());
    let spider_stopping = Arc::new(Notify::new());
    let release_spider = Arc::new(Notify::new());
    let spider_joined = Arc::new(AtomicBool::new(false));
    let mut spider = MockSpiderService::new();
    let (entered, stopping, release, joined) = (
        spider_entered.clone(),
        spider_stopping.clone(),
        release_spider.clone(),
        spider_joined.clone(),
    );
    spider
        .expect_run_until()
        .once()
        .returning(move |_, _, _, _, mut stop| {
            let (entered, stopping, release, joined) = (
                entered.clone(),
                stopping.clone(),
                release.clone(),
                joined.clone(),
            );
            Box::pin(async move {
                entered.notify_one();
                wait_for_stop(&mut stop).await;
                stopping.notify_one();
                release.notified().await;
                joined.store(true, Ordering::SeqCst);
                Err(crate::spider::service::SpiderServiceError::Cancelled)
            })
        });
    let mut spider_candidates = MockSpiderCandidateService::new();
    spider_candidates
        .expect_get_candidates()
        .once()
        .returning(|_, _| {
            Box::pin(async {
                Ok(vec![crate::spider::candidate_service::SpiderCandidate {
                    listing_source_id: ListingSourceId::new(),
                    domain_id: crate::CrawlerDomainId::new(),
                    listing_source_domain: "example.test".into(),
                    crawl_failure_count: 0,
                    last_crawl_error_kind: None,
                }])
            })
        });
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .once()
        .return_once(move |_, url, _, _, _, _| {
            let raw_input = crawler_verified_removal_input(url).unwrap();
            Box::pin(async move {
                spider_entered.notified().await;
                assert!(!worker_panic, "injected scraper worker panic");
                Ok(Some(ScrapedProduct {
                    raw_input,
                    availability:
                        product_listing_normalization::ListingAvailabilityQuickCheck::NoAssertion,
                    hash: "page".into(),
                    schema_fingerprint: "schema".into(),
                    raw_input_sha256: vec![3; 32],
                }))
            })
        });
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .times(if worker_panic { 1 } else { 2 })
        .returning(|_, _, excluded| {
            let first = excluded.is_empty();
            Box::pin(async move {
                Ok(if first {
                    vec![scraper_candidate(
                        "test",
                        "https://example.test/product".parse().unwrap(),
                    )]
                } else {
                    vec![]
                })
            })
        });
    let mut capture = MockProductListingRawCaptureService::new();
    capture
        .expect_capture()
        .times(usize::from(!worker_panic))
        .returning(|_| Box::pin(async { vec![ProductListingRawCaptureOutcome::RetryableFailure] }));
    let mut job = job();
    job.config.spider_concurrency = 1;
    job.config.scraper_concurrency = 1;
    job.spider_candidates = Arc::new(spider_candidates);
    job.spider_service = Arc::new(spider);
    job.scraper_candidates = Arc::new(candidates);
    job.scraper_service = Arc::new(scraper);
    job.raw_capture = Arc::new(capture);
    let returned = AtomicBool::new(false);
    let (_stop_tx, stop) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(
            async {
                let result = job.run_until(stop).await;
                returned.store(true, Ordering::SeqCst);
                result
            },
            async {
                spider_stopping.notified().await;
                assert!(
                    !returned.load(Ordering::SeqCst),
                    "must retain sibling cleanup join"
                );
                release_spider.notify_one();
            }
        )
    })
    .await;
    assert_eq!(result, Err(CrawlerRunError::ScraperPassIncomplete));
    assert!(spider_joined.load(Ordering::SeqCst));
}

#[rstest::rstest]
#[case::true_signal(false)]
#[case::sender_loss(true)]
#[tokio::test]
async fn should_do_no_work_when_initially_stopped(#[case] sender_loss: bool) {
    let (stop_tx, stop) = watch::channel(!sender_loss);
    let keep_signal = (!sender_loss).then(|| stop_tx.clone());
    drop(stop_tx);
    let mut job = job();
    job.config.spider_concurrency = 1;
    job.config.scraper_concurrency = 1;
    job.listing_source_registration = registration(
        MockListingSourceRegistrationSource::new(),
        MockListingSourceRegistrationRepository::new(),
    );
    assert_eq!(bounded(job.run_until(stop)).await, Ok(()));
    drop(keep_signal);
}

#[rstest::rstest]
#[case::source_read(false, false)]
#[case::snapshot_write(true, false)]
#[case::sender_loss(false, true)]
#[tokio::test]
async fn should_cancel_initial_sync_without_admitting_stale_scope(
    #[case] snapshot_pending: bool,
    #[case] sender_loss: bool,
) {
    let pending = PendingWork::default();
    let pending_source = pending.clone();
    let mut source = MockListingSourceRegistrationSource::new();
    source
        .expect_fetch_registered_listing_sources()
        .once()
        .returning(move || {
            let pending = pending_source.clone();
            Box::pin(async move {
                if snapshot_pending {
                    Ok(vec![])
                } else {
                    pending.wait().await
                }
            })
        });
    let mut repository = MockListingSourceRegistrationRepository::new();
    if snapshot_pending {
        let pending = pending.clone();
        repository
            .expect_apply_snapshot()
            .once()
            .returning(move |_| {
                let pending = pending.clone();
                Box::pin(async move { pending.wait().await })
            });
    }
    let mut job = job();
    job.config.spider_concurrency = 1;
    job.config.scraper_concurrency = 1;
    job.listing_source_registration = registration(source, repository);
    let (stop_tx, stop) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(job.run_until(stop), async {
            pending.entered.notified().await;
            if !sender_loss {
                stop_tx.send_replace(true);
            }
            drop(stop_tx);
        })
    })
    .await;
    assert_eq!(result, Ok(()));
    assert!(pending.dropped.load(Ordering::SeqCst));
}

#[rstest::rstest]
#[case::source_error(false, false)]
#[case::snapshot_error(true, false)]
#[case::error_with_stop(false, true)]
#[tokio::test]
async fn should_reject_initial_sync_error_even_when_stop_is_ready(
    #[case] snapshot_error: bool,
    #[case] stop_with_error: bool,
) {
    let (stop_tx, stop) = watch::channel(false);
    let signal = stop_tx.clone();
    let mut source = MockListingSourceRegistrationSource::new();
    source
        .expect_fetch_registered_listing_sources()
        .once()
        .returning(move || {
            if stop_with_error {
                signal.send_replace(true);
            }
            Box::pin(async move {
                if snapshot_error {
                    Ok(vec![])
                } else {
                    Err(ListingSourceSyncError::FetchError(
                        "secret-provider-body".into(),
                    ))
                }
            })
        });
    let mut repository = MockListingSourceRegistrationRepository::new();
    if snapshot_error {
        repository.expect_apply_snapshot().once().returning(|_| {
            Box::pin(async { Err(sqlx::Error::Protocol("secret-database-body".into())) })
        });
    }
    let mut job = job();
    job.config.spider_concurrency = 1;
    job.config.scraper_concurrency = 1;
    job.listing_source_registration = registration(source, repository);
    let error = bounded(job.run_until(stop)).await.unwrap_err();
    assert_eq!(error, CrawlerRunError::ListingSourceSyncFailed);
    assert!(!format!("{error} {error:?} {error:#?}").contains("secret"));
    assert!(std::error::Error::source(&error).is_none());
    drop(stop_tx);
}

#[tokio::test]
async fn should_stop_idle_loops_without_starting_another_pass() {
    let selected = Arc::new(Notify::new());
    let count = Arc::new(AtomicUsize::new(0));
    let mut spider_candidates = MockSpiderCandidateService::new();
    let notify = selected.clone();
    let calls = count.clone();
    spider_candidates
        .expect_get_candidates()
        .once()
        .returning(move |_, _| {
            if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                notify.notify_one();
            }
            Box::pin(async { Ok(vec![]) })
        });
    let mut scraper_candidates = MockScraperCandidateService::new();
    let calls = count.clone();
    let notify = selected.clone();
    scraper_candidates
        .expect_get_candidates()
        .once()
        .returning(move |_, _, _| {
            if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                notify.notify_one();
            }
            Box::pin(async { Ok(vec![]) })
        });
    let mut job = job();
    job.config.spider_concurrency = 1;
    job.config.scraper_concurrency = 1;
    job.spider_candidates = Arc::new(spider_candidates);
    job.scraper_candidates = Arc::new(scraper_candidates);
    let (stop_tx, stop) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(job.run_until(stop), async {
            selected.notified().await;
            stop_tx.send_replace(false);
            tokio::task::yield_now().await;
            stop_tx.send_replace(true);
        })
    })
    .await;
    assert_eq!(result, Ok(()));
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[rstest::rstest]
#[case::spider(true)]
#[case::scraper(false)]
#[tokio::test]
async fn should_reject_pass_scope_refresh_failure_without_selecting_candidates(
    #[case] spider: bool,
) {
    let calls = AtomicUsize::new(0);
    let mut source = MockListingSourceRegistrationSource::new();
    source
        .expect_fetch_registered_listing_sources()
        .times(2)
        .returning(move || {
            let initial = calls.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                if initial {
                    Ok(vec![])
                } else {
                    Err(ListingSourceSyncError::FetchError("unavailable".into()))
                }
            })
        });
    let mut job = job();
    job.config.spider_concurrency = usize::from(spider);
    job.config.scraper_concurrency = usize::from(!spider);
    job.listing_source_registration = registration(source, snapshot_repository());
    let (_stop_tx, stop) = watch::channel(false);
    let expected = if spider {
        CrawlerRunError::ListingSourceSyncFailed
    } else {
        CrawlerRunError::ScraperPassIncomplete
    };
    assert_eq!(bounded(job.run_until(stop)).await, Err(expected));
}

#[rstest::rstest]
#[case::pending_read(false)]
#[case::pending_snapshot(true)]
#[tokio::test(start_paused = true)]
async fn should_cancel_periodic_sync_and_join_before_return(#[case] snapshot_pending: bool) {
    let pending = PendingWork::default();
    let pending_source = pending.clone();
    let calls = AtomicUsize::new(0);
    let mut source = MockListingSourceRegistrationSource::new();
    source
        .expect_fetch_registered_listing_sources()
        .times(2)
        .returning(move || {
            let periodic = calls.fetch_add(1, Ordering::SeqCst) > 0;
            let pending = pending_source.clone();
            Box::pin(async move {
                if periodic && !snapshot_pending {
                    pending.wait().await
                } else {
                    Ok(vec![])
                }
            })
        });
    let mut repository = snapshot_repository();
    if snapshot_pending {
        repository = MockListingSourceRegistrationRepository::new();
        let calls = AtomicUsize::new(0);
        let pending = pending.clone();
        repository
            .expect_apply_snapshot()
            .times(2)
            .returning(move |_| {
                let periodic = calls.fetch_add(1, Ordering::SeqCst) > 0;
                let pending = pending.clone();
                Box::pin(async move {
                    if periodic {
                        pending.wait().await
                    } else {
                        Ok(ListingSourceSnapshotResult::default())
                    }
                })
            });
    }
    let mut job = job();
    job.config.listing_source_sync_interval = Duration::from_millis(10);
    job.listing_source_registration = registration(source, repository);
    let (stop_tx, stop) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(job.run_until(stop), async {
            pending.entered.notified().await;
            stop_tx.send_replace(true);
        })
    })
    .await;
    assert_eq!(result, Ok(()));
    assert!(pending.dropped.load(Ordering::SeqCst));
}

#[rstest::rstest]
#[case::error(false)]
#[case::panic(true)]
#[tokio::test(start_paused = true)]
async fn should_stop_and_join_active_sibling_when_periodic_sync_fails(#[case] panic: bool) {
    let fetch = PendingWork::default();
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .once()
        .returning(|_, _, _| {
            Box::pin(async {
                Ok(vec![scraper_candidate(
                    "test",
                    "https://example.test/active".parse().unwrap(),
                )])
            })
        });
    let pending = fetch.clone();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .once()
        .returning(move |_, _, _, _, _, _| {
            let pending = pending.clone();
            Box::pin(async move { pending.wait().await })
        });
    let calls = AtomicUsize::new(0);
    let mut source = MockListingSourceRegistrationSource::new();
    source
        .expect_fetch_registered_listing_sources()
        .times(3)
        .returning(move || {
            let periodic = calls.fetch_add(1, Ordering::SeqCst) == 2;
            Box::pin(async move {
                if periodic {
                    assert!(!panic, "injected sync panic");
                    Err(ListingSourceSyncError::FetchError("unavailable".into()))
                } else {
                    Ok(vec![])
                }
            })
        });
    let mut job = job();
    job.config.scraper_concurrency = 1;
    job.config.listing_source_sync_interval = Duration::from_millis(10);
    job.listing_source_registration = registration(source, snapshot_repository());
    job.scraper_candidates = Arc::new(candidates);
    job.scraper_service = Arc::new(scraper);
    let (_stop_tx, stop) = watch::channel(false);
    let expected = if panic {
        CrawlerRunError::LoopTaskFailed
    } else {
        CrawlerRunError::ListingSourceSyncFailed
    };
    assert_eq!(bounded(job.run_until(stop)).await, Err(expected));
    assert!(
        fetch.dropped.load(Ordering::SeqCst),
        "active sibling must be joined"
    );
}

#[rstest::rstest]
#[case::durable(0, false)]
#[case::capture_failed(1, false)]
#[case::collector_panicked(2, false)]
#[case::local_mark_failed(3, false)]
#[case::capture_count_mismatch(4, false)]
#[case::sender_loss(0, true)]
#[tokio::test]
async fn should_drain_accepted_capture_and_cancel_fetch_before_return(
    #[case] failure: u8,
    #[case] sender_loss: bool,
) {
    let fetch = PendingWork::default();
    let pending = fetch.clone();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .times(2)
        .returning(move |_, url, _, _, _, _| {
            let pending = pending.clone();
            let first = url.path() == "/accepted";
            let raw_input = crawler_verified_removal_input(url).unwrap();
            Box::pin(async move {
                if !first {
                    return pending.wait().await;
                }
                Ok(Some(ScrapedProduct {
                    raw_input,
                    availability:
                        product_listing_normalization::ListingAvailabilityQuickCheck::NoAssertion,
                    hash: "page".into(),
                    schema_fingerprint: "schema".into(),
                    raw_input_sha256: vec![3; 32],
                }))
            })
        });
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .once()
        .returning(|_, _, _| {
            Box::pin(async {
                Ok(["accepted", "interrupted", "never-fetched"]
                    .map(|path| {
                        scraper_candidate(
                            "test",
                            format!("https://example.test/{path}").parse().unwrap(),
                        )
                    })
                    .into())
            })
        });
    let marked = Arc::new(AtomicBool::new(false));
    let mark = marked.clone();
    candidates
        .expect_mark_as_scraped()
        .times(usize::from(failure == 0 || failure == 3))
        .returning(move |_, _, _, _, _, _, _| {
            mark.store(true, Ordering::SeqCst);
            Box::pin(async move {
                if failure == 3 {
                    Err(sqlx::Error::PoolClosed)
                } else {
                    Ok(CrawlerUrlWriteOutcome::Applied)
                }
            })
        });
    let capture_entered = Arc::new(Notify::new());
    let capture_release = Arc::new(Notify::new());
    let entered = capture_entered.clone();
    let release = capture_release.clone();
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().return_once(move |items| {
        assert_eq!(items.len(), 1);
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            match failure {
                1 => vec![ProductListingRawCaptureOutcome::RetryableFailure],
                2 => panic!("injected collector panic"),
                4 => vec![],
                _ => vec![ProductListingRawCaptureOutcome::Persisted],
            }
        })
    });
    let mut job = job();
    job.config.scraper_concurrency = 1;
    job.config.push_max_batch_age = Duration::from_secs(86400);
    job.scraper_candidates = Arc::new(candidates);
    job.scraper_service = Arc::new(scraper);
    job.raw_capture = Arc::new(capture);
    let returned = AtomicBool::new(false);
    let (stop_tx, stop) = watch::channel(false);
    let (result, ()) = bounded(async {
        tokio::join!(
            async {
                let result = job.run_until(stop).await;
                returned.store(true, Ordering::SeqCst);
                result
            },
            async {
                fetch.entered.notified().await;
                let stop_tx = if sender_loss {
                    drop(stop_tx);
                    None
                } else {
                    stop_tx.send_replace(true);
                    Some(stop_tx)
                };
                capture_entered.notified().await;
                assert!(fetch.dropped.load(Ordering::SeqCst));
                assert!(
                    !returned.load(Ordering::SeqCst),
                    "accepted collector must drain"
                );
                // A later caller mistake cannot clear the scheduler's latched stop.
                if let Some(signal) = &stop_tx {
                    signal.send_replace(false);
                }
                tokio::task::yield_now().await;
                capture_release.notify_one();
            }
        )
    })
    .await;
    let expected = if failure == 0 {
        Ok(())
    } else {
        Err(CrawlerRunError::ScraperPassIncomplete)
    };
    assert_eq!(result, expected);
    assert_eq!(marked.load(Ordering::SeqCst), failure == 0 || failure == 3);
}

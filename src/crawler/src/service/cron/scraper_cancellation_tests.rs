use super::*;
use futures::poll;
use tokio::sync::Notify;

#[derive(Clone, Default)]
struct PendingWork {
    entered: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl PendingWork {
    async fn wait<T>(&self) -> T {
        let _guard = DropFlag(self.dropped.clone());
        self.entered.notify_one();
        std::future::pending().await
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("fake work must stop/join without waiting for its provider")
}

fn candidate(path: &str) -> ScraperCandidate {
    scraper_candidate(
        "Cancellation source",
        url::Url::parse(&format!("https://example.test/{path}")).expect("test URL"),
    )
}

fn scraped(url: &url::Url) -> ScrapedProduct {
    ScrapedProduct {
        raw_input: crawler_verified_removal_input(url).expect("test raw input"),
        availability: product_listing_normalization::ListingAvailabilityQuickCheck::NoAssertion,
        hash: "page".into(),
        schema_fingerprint: "schema".into(),
        raw_input_sha256: vec![3; 32],
    }
}

fn select_once(candidates: Vec<ScraperCandidate>) -> MockScraperCandidateService {
    let mut service = MockScraperCandidateService::new();
    service
        .expect_get_candidates()
        .once()
        .return_once(move |_, _, _| Box::pin(async move { Ok(candidates) }));
    service.expect_touch_scraped().never();
    service
}

#[rstest::rstest]
#[case::true_signal(false)]
#[case::sender_loss(true)]
#[tokio::test]
async fn should_not_refresh_scope_or_select_candidates_when_already_stopped(
    #[case] sender_loss: bool,
) {
    let (stop_tx, stop) = watch::channel(!sender_loss);
    let _keep_signal = if sender_loss {
        None
    } else {
        Some(stop_tx.clone())
    };
    drop(stop_tx);
    let mut candidates = MockScraperCandidateService::new();
    candidates.expect_get_candidates().never();
    let mut source = MockListingSourceRegistrationSource::new();
    source.expect_fetch_registered_listing_sources().never();
    let mut job = scraper_job(
        CrawlerCronConfig::default(),
        candidates,
        MockScraperService::new(),
    );
    job.listing_source_registration = Arc::new(ListingSourceRegistrationService::new(
        Box::new(source),
        Box::new(MockListingSourceRegistrationRepository::new()),
    ));

    let outcome = bounded(job.run_scraper_pass_until(stop)).await;
    assert!(outcome.admission_stopped());
    assert!(outcome.is_complete());
    assert_eq!(outcome.total, 0);
    assert_eq!(outcome.accepted, 0);
}

#[tokio::test]
async fn should_cancel_pending_scope_refresh_without_selecting_candidates() {
    let pending = PendingWork::default();
    let pending_for_mock = pending.clone();
    let mut source = MockListingSourceRegistrationSource::new();
    source
        .expect_fetch_registered_listing_sources()
        .once()
        .returning(move || {
            let pending = pending_for_mock.clone();
            Box::pin(async move { pending.wait().await })
        });
    let mut candidates = MockScraperCandidateService::new();
    candidates.expect_get_candidates().never();
    let mut job = scraper_job(
        CrawlerCronConfig::default(),
        candidates,
        MockScraperService::new(),
    );
    job.listing_source_registration = Arc::new(ListingSourceRegistrationService::new(
        Box::new(source),
        Box::new(MockListingSourceRegistrationRepository::new()),
    ));
    let (stop_tx, stop) = watch::channel(false);
    let pass = job.run_scraper_pass_until(stop);
    tokio::pin!(pass);
    assert!(poll!(&mut pass).is_pending());
    stop_tx.send(true).expect("pass owns stop receiver");

    let outcome = bounded(pass).await;
    assert!(pending.dropped.load(Ordering::SeqCst));
    assert!(outcome.admission_stopped());
    assert!(outcome.is_complete());
}

#[tokio::test]
async fn should_cancel_pending_candidate_lookup_on_stop() {
    let pending = PendingWork::default();
    let pending_for_mock = pending.clone();
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .once()
        .returning(move |_, _, _| {
            let pending = pending_for_mock.clone();
            Box::pin(async move { pending.wait().await })
        });
    let job = scraper_job(
        CrawlerCronConfig::default(),
        candidates,
        MockScraperService::new(),
    );
    let (stop_tx, stop) = watch::channel(false);
    let pass = job.run_scraper_pass_until(stop);
    tokio::pin!(pass);
    assert!(poll!(&mut pass).is_pending());
    stop_tx.send(true).expect("pass owns stop receiver");

    let outcome = bounded(pass).await;
    assert!(pending.dropped.load(Ordering::SeqCst));
    assert!(outcome.admission_stopped());
    assert!(outcome.is_complete());
    assert_eq!(outcome.total, 0);
}

#[tokio::test]
async fn should_not_admit_returned_domains_or_refill_after_stop() {
    let (stop_tx, stop) = watch::channel(false);
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .once()
        .returning(move |_, _, _| {
            let stop_tx = stop_tx.clone();
            Box::pin(async move {
                stop_tx.send(true).expect("pass owns stop receiver");
                Ok(vec![candidate("not-admitted")])
            })
        });
    let mut scraper = MockScraperService::new();
    scraper.expect_scrape().never();
    let job = scraper_job(CrawlerCronConfig::default(), candidates, scraper);

    let outcome = bounded(job.run_scraper_pass_until(stop)).await;
    assert!(outcome.admission_stopped());
    assert!(outcome.is_complete());
    assert_eq!(outcome.total, 0);
}

#[rstest::rstest]
#[case::fetch_stop(false, false)]
#[case::fetch_sender_loss(false, true)]
#[case::schema_generation_stop(true, false)]
#[tokio::test]
async fn should_cancel_active_fetch_or_schema_generation_and_join_before_return(
    #[case] schema_generation: bool,
    #[case] sender_loss: bool,
) {
    use crate::scraper::css_selector::product_schema_service::MockProductListingSchemaService;
    use crate::scraper::normalization::product_normalization_service::MockProductListingNormalizationService;
    use crate::scraper::scraper_service::service::{
        FetchedHtml, MockHtmlFetcher, ScraperServiceImpl,
    };

    let fetched_candidate = candidate("blocked-fetch");
    let listing_source_id = fetched_candidate.listing_source_id;
    let url = fetched_candidate.url.clone();
    let mut candidates = select_once(vec![fetched_candidate, candidate("must-not-fetch")]);
    candidates.expect_mark_as_scraped().never();
    candidates.expect_mark_removed().never();
    candidates.expect_mark_fetch_failure().never();
    candidates.expect_mark_scraper_failure().never();
    let pending = PendingWork::default();
    let pending_for_mock = pending.clone();
    let mut fetcher = MockHtmlFetcher::new();
    fetcher.expect_fetch().once().returning(move |url| {
        let pending = pending_for_mock.clone();
        let url = url.clone();
        Box::pin(async move {
            if schema_generation {
                Ok(FetchedHtml::new("<main>test product</main>".into(), url))
            } else {
                pending.wait().await
            }
        })
    });
    let mut schemas = MockProductListingSchemaService::new();
    if schema_generation {
        schemas
            .expect_find_product_schema()
            .once()
            .returning(|_| Box::pin(async { Ok(None) }));
        let pending_for_mock = pending.clone();
        schemas
            .expect_create_product_schemas()
            .once()
            .returning(move |_| {
                let pending = pending_for_mock.clone();
                Box::pin(async move { pending.wait().await })
            });
        candidates
            .expect_try_increment_listing_source_llm_calls_with_limit()
            .once()
            .returning(|_, _, _| Box::pin(async { Ok(true) }));
    }
    let mut job = scraper_job(
        CrawlerCronConfig {
            scraper_concurrency: 1,
            ..Default::default()
        },
        candidates,
        MockScraperService::new(),
    );
    job.scraper_service = Arc::new(ScraperServiceImpl::new_with_schema_seed_pages(
        Box::new(fetcher),
        Box::new(schemas),
        Box::new(MockProductListingNormalizationService::new()),
        job.scraper_candidates.clone(),
        1,
        20,
    ));
    let (stop_tx, stop) = watch::channel(false);
    let (outcome, ()) = bounded(async {
        tokio::join!(job.run_scraper_pass_until(stop), async {
            pending.entered.notified().await;
            if !sender_loss {
                stop_tx.send(true).expect("pass owns stop receiver");
            }
            drop(stop_tx);
        })
    })
    .await;

    assert!(pending.dropped.load(Ordering::SeqCst));
    assert!(UrlLock::try_acquire(&job.lock_manager, &url).is_some());
    assert!(ListingSourceLock::try_acquire(&job.lock_manager, listing_source_id).is_some());
    assert!(outcome.admission_stopped());
    assert!(outcome.is_complete());
    assert_eq!(outcome.accepted, 0);
    assert_eq!(outcome.captures.completed, 0);
}

#[rstest::rstest]
#[case::true_signal(false)]
#[case::sender_loss(true)]
#[tokio::test]
async fn should_stop_pending_enqueue_and_leave_fetched_unaccepted_work_retryable(
    #[case] sender_loss: bool,
) {
    let mut candidates = MockScraperCandidateService::new();
    candidates.expect_mark_as_scraped().never();
    candidates.expect_touch_scraped().never();
    candidates.expect_mark_removed().never();
    candidates.expect_mark_fetch_failure().never();
    candidates.expect_mark_scraper_failure().never();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .times(2)
        .returning(|_, url, _, _, _, _| {
            let scraped = scraped(url);
            Box::pin(async move { Ok(Some(scraped)) })
        });
    let mut ctx = scrape_candidate_context(candidates, scraper);
    let (tx, mut rx) = mpsc::channel(1);
    ctx.command_tx = tx;
    let (stop_tx, stop) = watch::channel(false);
    let producer = scrape_domain_candidates(
        vec![
            candidate("accepted"),
            candidate("fetched-not-enqueued"),
            candidate("never-fetched"),
        ],
        ctx,
        stop,
    );
    tokio::pin!(producer);
    // Both fake fetches are ready. Only the second enqueue can block this producer.
    assert!(poll!(&mut producer).is_pending());
    assert_eq!(rx.len(), 1);
    if !sender_loss {
        stop_tx.send(true).expect("producer owns stop receiver");
    }
    drop(stop_tx);

    let outcome = bounded(producer).await;
    assert!(outcome.admission_stopped);
    assert_eq!(outcome.accepted, 1);
    assert_eq!(outcome.failed, 0);
    assert_eq!(outcome.skipped, 0);
    let accepted = rx.recv().await.expect("first capture stays accepted");
    assert!(
        accepted
            .request
            .item
            .command
            .source_record_key
            .ends_with("/accepted")
    );
    assert!(
        rx.recv().await.is_none(),
        "every producer sender must be dropped"
    );
}

#[rstest::rstest]
#[case::false_update(false)]
#[case::true_stop(true)]
#[tokio::test]
async fn should_respect_stop_value_when_capacity_becomes_ready(#[case] stopping: bool) {
    let (tx, mut rx) = mpsc::channel(1);
    let (stop_tx, mut stop) = watch::channel(false);
    let id = ListingSourceId::new();
    let request = || {
        (
            item(id, "stopped"),
            meta(id, "https://example.test/stopped", "page"),
        )
    };
    assert!(
        enqueue_raw_capture(&tx, request(), &mut stop)
            .await
            .unwrap()
            .is_some()
    );
    let second = enqueue_raw_capture(&tx, request(), &mut stop);
    tokio::pin!(second);
    assert!(poll!(&mut second).is_pending());
    stop_tx.send(stopping).expect("enqueue owns stop receiver");
    assert!(rx.recv().await.is_some());

    assert_eq!(bounded(second).await.unwrap().is_none(), stopping);
    assert_eq!(
        rx.try_recv().is_err(),
        stopping,
        "ready capacity must not outrank true stop"
    );
}

#[rstest::rstest]
#[case::durable(vec![ProductListingRawCaptureOutcome::Persisted], false, true, 0)]
#[case::capture_failure(vec![ProductListingRawCaptureOutcome::RetryableFailure], false, false, 0)]
#[case::missing_result(vec![], false, false, 1)]
#[case::extra_result(vec![ProductListingRawCaptureOutcome::Persisted; 2], false, false, 1)]
#[case::local_mark_failure(vec![ProductListingRawCaptureOutcome::Persisted], true, false, 0)]
#[tokio::test]
async fn should_join_producers_and_flush_accepted_partial_batch_on_stop(
    #[case] capture_results: Vec<ProductListingRawCaptureOutcome>,
    #[case] local_mark_failure: bool,
    #[case] complete: bool,
    #[case] mismatches: usize,
) {
    let persisted = capture_results.first() == Some(&ProductListingRawCaptureOutcome::Persisted);
    let pending_fetch = PendingWork::default();
    let pending_for_mock = pending_fetch.clone();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .times(2)
        .returning(move |_, url, _, _, _, _| {
            let pending = pending_for_mock.clone();
            let first = url.path() == "/accepted";
            let scraped = scraped(url);
            Box::pin(async move {
                if first {
                    Ok(Some(scraped))
                } else {
                    pending.wait().await
                }
            })
        });
    let confirmed = Arc::new(AtomicBool::new(false));
    let marks = Arc::new(AtomicUsize::new(0));
    let mut candidates = select_once(vec![
        candidate("accepted"),
        candidate("interrupted"),
        candidate("never-fetched"),
    ]);
    let confirmed_for_mock = confirmed.clone();
    let marks_for_mock = marks.clone();
    candidates
        .expect_mark_as_scraped()
        .times(usize::from(persisted))
        .returning(move |_, url, _, _, _, _, _| {
            assert_eq!(url.path(), "/accepted");
            assert!(
                confirmed_for_mock.load(Ordering::SeqCst),
                "local mark needs confirmed business capture"
            );
            marks_for_mock.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if local_mark_failure {
                    Err(sqlx::Error::PoolClosed)
                } else {
                    Ok(CrawlerUrlWriteOutcome::Applied)
                }
            })
        });
    candidates.expect_mark_removed().never();
    candidates.expect_mark_fetch_failure().never();
    candidates.expect_mark_scraper_failure().never();
    let capture_entered = Arc::new(Notify::new());
    let capture_release = Arc::new(Notify::new());
    let entered_for_mock = capture_entered.clone();
    let release_for_mock = capture_release.clone();
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().return_once(move |items| {
        assert_eq!(
            items.len(),
            1,
            "stop must flush the accepted partial batch only"
        );
        Box::pin(async move {
            entered_for_mock.notify_one();
            release_for_mock.notified().await;
            confirmed.store(persisted, Ordering::SeqCst);
            capture_results
        })
    });
    let mut job = scraper_job(
        CrawlerCronConfig {
            scraper_concurrency: 1,
            push_batch_size: 25,
            push_max_batch_age: Duration::from_secs(86400),
            ..Default::default()
        },
        candidates,
        scraper,
    );
    job.raw_capture = Arc::new(capture);
    let (stop_tx, stop) = watch::channel(false);
    let (outcome, ()) = bounded(async {
        tokio::join!(job.run_scraper_pass_until(stop), async {
            pending_fetch.entered.notified().await;
            stop_tx.send(true).expect("pass owns stop receiver");
            capture_entered.notified().await;
            assert!(pending_fetch.dropped.load(Ordering::SeqCst));
            assert_eq!(
                marks.load(Ordering::SeqCst),
                0,
                "enqueue and drain start are not durable"
            );
            capture_release.notify_one();
        })
    })
    .await;

    assert!(outcome.admission_stopped());
    assert_eq!(outcome.is_complete(), complete);
    assert_eq!(outcome.accepted, 1);
    assert_eq!(outcome.captures.accepted, 1);
    assert_eq!(outcome.captures.durable, usize::from(persisted));
    assert_eq!(
        outcome.captures.completed,
        usize::from(persisted && !local_mark_failure)
    );
    assert_eq!(outcome.captures.capture_failed, usize::from(!persisted));
    assert_eq!(
        outcome.captures.local_mark_failed,
        usize::from(local_mark_failure)
    );
    assert_eq!(outcome.captures.result_mismatches, mismatches);
    assert!(!outcome.worker_failed);
}

#[tokio::test]
async fn should_stop_backpressured_pass_then_drain_inflight_and_queued_captures() {
    let third_fetched = Arc::new(Notify::new());
    let third_for_mock = third_fetched.clone();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .times(3)
        .returning(move |_, url, _, _, _, _| {
            if url.path() == "/third" {
                third_for_mock.notify_one();
            }
            let scraped = scraped(url);
            Box::pin(async move { Ok(Some(scraped)) })
        });
    let mut candidates = select_once(vec![
        candidate("first"),
        candidate("second"),
        candidate("third"),
        candidate("never-fetched"),
    ]);
    let marks = Arc::new(AtomicUsize::new(0));
    let marks_for_mock = marks.clone();
    candidates
        .expect_mark_as_scraped()
        .times(2)
        .returning(move |_, url, _, _, _, _, _| {
            assert!(matches!(url.path(), "/first" | "/second"));
            marks_for_mock.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
        });
    let release = Arc::new(Notify::new());
    let release_for_mock = release.clone();
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().times(2).returning(move |items| {
        assert_eq!(items.len(), 1);
        let first = items[0].command.source_record_key.ends_with("/first");
        let release = release_for_mock.clone();
        Box::pin(async move {
            if first {
                release.notified().await;
            }
            vec![ProductListingRawCaptureOutcome::Persisted]
        })
    });
    let mut job = scraper_job(
        CrawlerCronConfig {
            scraper_concurrency: 1,
            push_queue_capacity: 1,
            push_batch_size: 1,
            ..Default::default()
        },
        candidates,
        scraper,
    );
    job.raw_capture = Arc::new(capture);
    let (stop_tx, stop) = watch::channel(false);
    let mut pass = Box::pin(job.run_scraper_pass_until(stop));
    bounded(async {
        tokio::select! {
            _ = third_fetched.notified() => {}
            _ = &mut pass => panic!("third enqueue must wait behind blocked capture and full queue"),
        }
    }).await;
    stop_tx.send(true).expect("pass owns stop receiver");
    assert!(
        poll!(&mut pass).is_pending(),
        "accepted captures still need confirmation"
    );
    assert_eq!(marks.load(Ordering::SeqCst), 0);
    release.notify_one();

    let outcome = bounded(pass).await;
    assert!(outcome.admission_stopped());
    assert!(outcome.is_complete());
    assert_eq!(outcome.accepted, 2);
    assert_eq!(outcome.captures.accepted, 2);
    assert_eq!(outcome.captures.completed, 2);
}

#[tokio::test]
async fn should_join_cancelled_sibling_and_drain_after_producer_panic() {
    let pending = PendingWork::default();
    let pending_for_mock = pending.clone();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .times(3)
        .returning(move |_, url, _, _, _, _| {
            let path = url.path().to_owned();
            let pending = pending_for_mock.clone();
            let scraped = scraped(url);
            Box::pin(async move {
                match path.as_str() {
                    "/accepted" => Ok(Some(scraped)),
                    "/panic" => {
                        pending.entered.notified().await;
                        panic!("injected producer failure");
                    }
                    _ => pending.wait().await,
                }
            })
        });
    let mut candidates = select_once(vec![
        candidate("accepted"),
        candidate("panic"),
        scraper_candidate(
            "Sibling",
            url::Url::parse("https://sibling.test/blocked").unwrap(),
        ),
    ]);
    candidates
        .expect_mark_as_scraped()
        .once()
        .returning(|_, _, _, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().returning(|items| {
        assert_eq!(items.len(), 1);
        Box::pin(async { vec![ProductListingRawCaptureOutcome::Persisted] })
    });
    let mut job = scraper_job(
        CrawlerCronConfig {
            scraper_concurrency: 2,
            push_max_batch_age: Duration::from_secs(86400),
            ..Default::default()
        },
        candidates,
        scraper,
    );
    job.raw_capture = Arc::new(capture);

    let outcome = bounded(job.run_scraper_pass()).await;
    assert!(
        pending.dropped.load(Ordering::SeqCst),
        "aborted sibling must be joined"
    );
    assert!(outcome.worker_failed);
    assert!(!outcome.is_complete());
    assert_eq!(outcome.captures.accepted, 1);
    assert_eq!(outcome.captures.completed, 1);
    assert!(!format!("{outcome:?}").contains("injected"));
}

#[tokio::test]
async fn should_stop_active_producer_when_collector_panics() {
    let pending = PendingWork::default();
    let pending_for_mock = pending.clone();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .times(2)
        .returning(move |_, url, _, _, _, _| {
            let pending = pending_for_mock.clone();
            let first = url.path() == "/accepted";
            let scraped = scraped(url);
            Box::pin(async move {
                if first {
                    Ok(Some(scraped))
                } else {
                    pending.wait().await
                }
            })
        });
    let mut candidates = select_once(vec![candidate("accepted"), candidate("blocked")]);
    candidates.expect_mark_as_scraped().never();
    candidates.expect_mark_removed().never();
    let entered = pending.entered.clone();
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().return_once(move |_| {
        Box::pin(async move {
            entered.notified().await;
            panic!("injected collector failure");
        })
    });
    let mut job = scraper_job(
        CrawlerCronConfig {
            scraper_concurrency: 1,
            push_batch_size: 1,
            ..Default::default()
        },
        candidates,
        scraper,
    );
    job.raw_capture = Arc::new(capture);

    let outcome = bounded(job.run_scraper_pass()).await;
    assert!(pending.dropped.load(Ordering::SeqCst));
    assert!(outcome.collector_failed);
    assert!(!outcome.is_complete());
    assert_eq!(outcome.captures.completed, 0);
    assert!(!format!("{outcome:?}").contains("injected"));
}

#[rstest::rstest]
#[case::lookup_error(false)]
#[case::admission_panic(true)]
#[tokio::test]
async fn should_join_producers_and_drain_when_candidate_lookup_fails(#[case] panic: bool) {
    let pending = PendingWork::default();
    let pending_for_scrape = pending.clone();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .times(2)
        .returning(move |_, url, _, _, _, _| {
            let pending = pending_for_scrape.clone();
            let first = url.path() == "/accepted";
            let scraped = scraped(url);
            Box::pin(async move {
                if first {
                    Ok(Some(scraped))
                } else {
                    pending.wait().await
                }
            })
        });
    let mut candidates = MockScraperCandidateService::new();
    let entered = pending.entered.clone();
    candidates
        .expect_get_candidates()
        .times(2)
        .returning(move |_, _, excluded| {
            let first = excluded.is_empty();
            let entered = entered.clone();
            Box::pin(async move {
                if first {
                    Ok(vec![candidate("accepted"), candidate("blocked")])
                } else {
                    entered.notified().await;
                    assert!(!panic, "injected admission failure");
                    Err(sqlx::Error::PoolClosed)
                }
            })
        });
    candidates
        .expect_mark_as_scraped()
        .once()
        .returning(|_, _, _, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().returning(|items| {
        assert_eq!(items.len(), 1);
        Box::pin(async { vec![ProductListingRawCaptureOutcome::Persisted] })
    });
    let mut job = scraper_job(
        CrawlerCronConfig {
            scraper_concurrency: 2,
            push_max_batch_age: Duration::from_secs(86400),
            ..Default::default()
        },
        candidates,
        scraper,
    );
    job.raw_capture = Arc::new(capture);

    let outcome = bounded(job.run_scraper_pass()).await;
    assert!(pending.dropped.load(Ordering::SeqCst));
    assert!(!outcome.is_complete());
    assert_eq!(outcome.candidate_lookup_failed, !panic);
    assert!(outcome.worker_failed);
    assert_eq!(outcome.captures.accepted, 1);
    assert_eq!(outcome.captures.completed, 1);
}

#[tokio::test]
async fn should_notice_producer_panic_while_candidate_lookup_is_pending() {
    let lookup = PendingWork::default();
    let lookup_for_mock = lookup.clone();
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .times(2)
        .returning(move |_, _, excluded| {
            let first = excluded.is_empty();
            let lookup = lookup_for_mock.clone();
            Box::pin(async move {
                if first {
                    Ok(vec![candidate("accepted"), candidate("panic")])
                } else {
                    lookup.wait().await
                }
            })
        });
    candidates
        .expect_mark_as_scraped()
        .once()
        .returning(|_, _, _, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
    let mut scraper = MockScraperService::new();
    let entered = lookup.entered.clone();
    scraper
        .expect_scrape()
        .times(2)
        .returning(move |_, url, _, _, _, _| {
            let first = url.path() == "/accepted";
            let scraped = scraped(url);
            let entered = entered.clone();
            Box::pin(async move {
                if first {
                    return Ok(Some(scraped));
                }
                entered.notified().await;
                panic!("injected producer failure during lookup");
            })
        });
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().returning(|items| {
        assert_eq!(items.len(), 1);
        Box::pin(async { vec![ProductListingRawCaptureOutcome::Persisted] })
    });
    let mut job = scraper_job(
        CrawlerCronConfig {
            scraper_concurrency: 2,
            ..Default::default()
        },
        candidates,
        scraper,
    );
    job.raw_capture = Arc::new(capture);

    let outcome = bounded(job.run_scraper_pass()).await;
    assert!(lookup.dropped.load(Ordering::SeqCst));
    assert!(outcome.worker_failed);
    assert!(!outcome.is_complete());
    assert_eq!(outcome.captures.accepted, 1);
    assert_eq!(outcome.captures.completed, 1);
}

#[tokio::test]
async fn should_keep_stop_receiver_and_await_capture_after_producers_finish() {
    let exhausted = Arc::new(Notify::new());
    let exhausted_for_mock = exhausted.clone();
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .times(2)
        .returning(move |_, _, excluded| {
            let first = excluded.is_empty();
            let exhausted = exhausted_for_mock.clone();
            Box::pin(async move {
                if first {
                    Ok(vec![candidate("accepted")])
                } else {
                    exhausted.notify_one();
                    Ok(vec![])
                }
            })
        });
    let marks = Arc::new(AtomicUsize::new(0));
    let marks_for_mock = marks.clone();
    candidates
        .expect_mark_as_scraped()
        .once()
        .returning(move |_, _, _, _, _, _, _| {
            marks_for_mock.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
        });
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .once()
        .returning(|_, url, _, _, _, _| {
            let scraped = scraped(url);
            Box::pin(async move { Ok(Some(scraped)) })
        });
    let entered = Arc::new(Notify::new());
    let entered_for_mock = entered.clone();
    let release = Arc::new(Notify::new());
    let release_for_mock = release.clone();
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().return_once(move |_| {
        Box::pin(async move {
            entered_for_mock.notify_one();
            release_for_mock.notified().await;
            vec![ProductListingRawCaptureOutcome::Persisted]
        })
    });
    let mut job = scraper_job(
        CrawlerCronConfig {
            scraper_concurrency: 1,
            push_batch_size: 1,
            ..Default::default()
        },
        candidates,
        scraper,
    );
    job.raw_capture = Arc::new(capture);
    let (stop_tx, stop) = watch::channel(false);
    let mut pass = Box::pin(job.run_scraper_pass_until(stop));
    bounded(async {
        tokio::select! {
            _ = async { entered.notified().await; exhausted.notified().await; } => {}
            _ = &mut pass => panic!("pass must await unconfirmed capture"),
        }
    })
    .await;
    assert!(poll!(&mut pass).is_pending());
    stop_tx
        .send(true)
        .expect("whole pass retains stop receiver through drain");
    assert!(
        poll!(&mut pass).is_pending(),
        "stop must not cancel accepted capture"
    );
    assert_eq!(marks.load(Ordering::SeqCst), 0);
    release.notify_one();

    let outcome = bounded(pass).await;
    assert!(outcome.admission_stopped());
    assert!(outcome.is_complete());
    assert_eq!(outcome.accepted, 1);
    assert_eq!(outcome.captures.completed, 1);
}

#[tokio::test]
async fn should_drop_owned_collector_instead_of_detaching_when_whole_pass_is_dropped() {
    let pending = PendingWork::default();
    let pending_for_mock = pending.clone();
    let mut capture = MockProductListingRawCaptureService::new();
    capture.expect_capture().once().returning(move |_| {
        let pending = pending_for_mock.clone();
        Box::pin(async move { pending.wait().await })
    });
    let mut candidates = MockScraperCandidateService::new();
    candidates
        .expect_get_candidates()
        .returning(get_candidates_once_by_domain(|| {
            vec![candidate("accepted")]
        }));
    candidates.expect_mark_as_scraped().never();
    candidates.expect_mark_removed().never();
    let mut scraper = MockScraperService::new();
    scraper
        .expect_scrape()
        .once()
        .returning(|_, url, _, _, _, _| {
            let scraped = scraped(url);
            Box::pin(async move { Ok(Some(scraped)) })
        });
    let mut job = scraper_job(
        CrawlerCronConfig {
            push_batch_size: 1,
            ..Default::default()
        },
        candidates,
        scraper,
    );
    job.raw_capture = Arc::new(capture);
    let mut pass = Box::pin(job.run_scraper_pass());
    bounded(async {
        tokio::select! {
            _ = pending.entered.notified() => {}
            _ = &mut pass => panic!("pass must await its pending collector"),
        }
    })
    .await;

    drop(pass);
    assert!(
        pending.dropped.load(Ordering::SeqCst),
        "collector must not outlive its pass"
    );
}

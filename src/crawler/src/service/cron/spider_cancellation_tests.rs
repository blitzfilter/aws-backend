use super::*;
use crate::service::cron::test_support::{GeneratedPatternFailure, PendingWork, bounded};

fn job(candidates: MockSpiderCandidateService, service: MockSpiderService) -> CrawlerCronJob {
    CrawlerCronJob::new(
        CrawlerCronConfig {
            spider_concurrency: 1,
            ..Default::default()
        },
        Arc::new(LocalLockManager::new()),
        Box::new(candidates),
        Box::new(service),
        Box::new(MockScraperCandidateService::new()),
        Box::new(MockScraperService::new()),
        noop_listing_source_registration(),
        noop_raw_capture(),
    )
}

fn candidates_for(id: CrawlerDomainId) -> MockSpiderCandidateService {
    let mut candidates = MockSpiderCandidateService::new();
    candidates
        .expect_get_candidates()
        .returning(move |_, excluded| {
            let unseen = !excluded.contains(&id);
            Box::pin(async move {
                Ok(if unseen {
                    vec![spider_candidate(id)]
                } else {
                    vec![]
                })
            })
        });
    candidates
}

fn success() -> SpiderRunResult {
    SpiderRunResult {
        total_links: 10,
        product_urls_count: 5,
        product_pattern: None,
    }
}

#[tokio::test]
async fn should_do_no_work_when_pass_initially_stopped() {
    let (stop_tx, stop) = watch::channel(true);
    let mut job = job(MockSpiderCandidateService::new(), MockSpiderService::new());
    job.listing_source_registration = Arc::new(ListingSourceRegistrationService::new(
        Box::new(MockListingSourceRegistrationSource::new()),
        Box::new(MockListingSourceRegistrationRepository::new()),
    ));
    let outcome = bounded(job.run_spider_pass_until(stop, &stop_tx)).await;
    assert!(outcome.admission_stopped());
    assert_eq!(outcome.into_result(), Ok(()));
}

#[rstest::rstest]
#[case::scope_refresh(true)]
#[case::candidate_lookup(false)]
#[tokio::test]
async fn should_cancel_read_only_selection_on_stop(#[case] scope: bool) {
    let pending = PendingWork::default();
    let mut candidates = MockSpiderCandidateService::new();
    if !scope {
        let pending = pending.clone();
        candidates
            .expect_get_candidates()
            .once()
            .returning(move |_, _| {
                let pending = pending.clone();
                Box::pin(async move { pending.wait().await })
            });
    }
    let mut job = job(candidates, MockSpiderService::new());
    if scope {
        let pending = pending.clone();
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .once()
            .returning(move || {
                let pending = pending.clone();
                Box::pin(async move { pending.wait().await })
            });
        job.listing_source_registration = Arc::new(ListingSourceRegistrationService::new(
            Box::new(source),
            Box::new(MockListingSourceRegistrationRepository::new()),
        ));
    }
    let (stop_tx, stop) = watch::channel(false);
    let (outcome, ()) = bounded(async {
        tokio::join!(job.run_spider_pass_until(stop, &stop_tx), async {
            pending.entered.notified().await;
            stop_tx.send_replace(true);
        })
    })
    .await;
    assert!(outcome.admission_stopped());
    assert_eq!(outcome.into_result(), Ok(()));
    assert!(pending.dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_not_admit_returned_candidates_or_refill_after_stop() {
    let (stop_tx, stop) = watch::channel(false);
    let signal = stop_tx.clone();
    let mut candidates = MockSpiderCandidateService::new();
    candidates
        .expect_get_candidates()
        .once()
        .returning(move |_, _| {
            signal.send_replace(true);
            Box::pin(async { Ok(vec![spider_candidate(CrawlerDomainId::new())]) })
        });
    let job = job(candidates, MockSpiderService::new());
    let outcome = bounded(job.run_spider_pass_until(stop, &stop_tx)).await;
    assert!(outcome.admission_stopped());
    assert_eq!(outcome.into_result(), Ok(()));
}

#[tokio::test]
async fn should_join_cancelled_service_without_cooldown_or_reset() {
    let entered = Arc::new(Notify::new());
    let stopping = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let mut service = MockSpiderService::new();
    let started = entered.clone();
    let stopped = stopping.clone();
    let finish = release.clone();
    service
        .expect_run_until()
        .once()
        .returning(move |_, _, _, _, mut stop| {
            let started = started.clone();
            let stopped = stopped.clone();
            let finish = finish.clone();
            Box::pin(async move {
                started.notify_one();
                wait_for_stop(&mut stop).await;
                stopped.notify_one();
                finish.notified().await;
                assert!(*stop.borrow(), "scheduler stop must stay latched");
                Err(SpiderServiceError::Cancelled)
            })
        });
    let id = CrawlerDomainId::new();
    let mut candidates = MockSpiderCandidateService::new();
    candidates
        .expect_get_candidates()
        .once()
        .return_once(move |_, _| Box::pin(async move { Ok(vec![spider_candidate(id)]) }));
    let job = job(candidates, service);
    let returned = AtomicBool::new(false);
    let (stop_tx, stop) = watch::channel(false);
    let (outcome, ()) = bounded(async {
        tokio::join!(
            async {
                let outcome = job.run_spider_pass_until(stop, &stop_tx).await;
                returned.store(true, Ordering::SeqCst);
                outcome
            },
            async {
                entered.notified().await;
                stop_tx.send_replace(true);
                stopping.notified().await;
                assert!(!returned.load(Ordering::SeqCst));
                assert!(DomainLock::try_acquire(&job.lock_manager, id).is_none());
                release.notify_one();
            }
        )
    })
    .await;
    assert!(outcome.admission_stopped());
    assert_eq!(outcome.into_result(), Ok(()));
    assert!(DomainLock::try_acquire(&job.lock_manager, id).is_some());
}

#[tokio::test]
async fn should_reject_unsolicited_service_cancellation() {
    let mut candidates = candidates_for(CrawlerDomainId::new());
    candidates
        .expect_mark_crawl_failure()
        .once()
        .withf(|_, kind, count, next| {
            kind == "spider_run_error"
                && *count == 1
                && next_crawl_at_is_about(
                    *next,
                    durable_retry_cooldown_for(NetworkErrorKind::Unknown),
                )
        })
        .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
    let mut service = MockSpiderService::new();
    service
        .expect_run_until()
        .once()
        .returning(|_, _, _, _, _| Box::pin(async { Err(SpiderServiceError::Cancelled) }));
    let job = job(candidates, service);
    let (stop_tx, stop) = watch::channel(false);
    let outcome = bounded(job.run_spider_pass_until(stop, &stop_tx)).await;
    assert_eq!(
        outcome.into_result(),
        Err(CrawlerRunError::SpiderServiceFailed)
    );
    assert!(*stop_tx.borrow());
}

#[rstest::rstest]
#[case::reset(true, false)]
#[case::mark(false, false)]
#[case::reset_at_stop(true, true)]
#[case::mark_at_stop(false, true)]
#[tokio::test]
async fn should_not_report_success_when_failure_metadata_write_fails(
    #[case] reset: bool,
    #[case] stop_during_write: bool,
) {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let started = entered.clone();
    let finish = release.clone();
    let mut candidates = candidates_for(CrawlerDomainId::new());
    if reset {
        candidates
            .expect_reset_crawl_failure()
            .once()
            .returning(move |_| {
                let started = started.clone();
                let finish = finish.clone();
                Box::pin(async move {
                    started.notify_one();
                    if stop_during_write {
                        finish.notified().await;
                    }
                    Err(sqlx::Error::PoolClosed)
                })
            });
    } else {
        candidates
            .expect_mark_crawl_failure()
            .once()
            .returning(move |_, _, _, _| {
                let started = started.clone();
                let finish = finish.clone();
                Box::pin(async move {
                    started.notify_one();
                    if stop_during_write {
                        finish.notified().await;
                    }
                    Err(sqlx::Error::PoolClosed)
                })
            });
    }
    let mut service = MockSpiderService::new();
    service
        .expect_run_until()
        .once()
        .returning(move |_, _, _, _, _| {
            Box::pin(async move {
                if reset {
                    Ok(success())
                } else {
                    Err(SpiderServiceError::EmptyCrawl {
                        crawl_root_url: "redacted".into(),
                    })
                }
            })
        });
    let job = job(candidates, service);
    let (stop_tx, stop) = watch::channel(false);
    let (outcome, ()) = bounded(async {
        tokio::join!(job.run_spider_pass_until(stop, &stop_tx), async {
            if stop_during_write {
                entered.notified().await;
                stop_tx.send_replace(true);
                release.notify_one();
            }
        })
    })
    .await;
    let expected = if reset {
        CrawlerRunError::SpiderFailureResetFailed
    } else {
        CrawlerRunError::SpiderFailureMarkFailed
    };
    assert_eq!(outcome.into_result(), Err(expected));
    assert!(*stop_tx.borrow());
}

#[rstest::rstest]
#[case::lookup_error(false)]
#[case::lookup_panic(true)]
#[tokio::test]
async fn should_join_admitted_service_when_candidate_selection_fails(#[case] panic: bool) {
    let id = CrawlerDomainId::new();
    let entered = Arc::new(Notify::new());
    let started = entered.clone();
    let mut candidates = MockSpiderCandidateService::new();
    candidates
        .expect_get_candidates()
        .times(2)
        .returning(move |_, excluded| {
            let first = excluded.is_empty();
            let started = started.clone();
            Box::pin(async move {
                if first {
                    return Ok(vec![spider_candidate(id)]);
                }
                started.notified().await;
                assert!(!panic, "injected candidate selection panic");
                Err(sqlx::Error::PoolClosed)
            })
        });
    let joined = Arc::new(AtomicBool::new(false));
    let finished = joined.clone();
    let mut service = MockSpiderService::new();
    service
        .expect_run_until()
        .once()
        .returning(move |_, _, _, _, mut stop| {
            let entered = entered.clone();
            let finished = finished.clone();
            Box::pin(async move {
                entered.notify_one();
                wait_for_stop(&mut stop).await;
                finished.store(true, Ordering::SeqCst);
                Err(SpiderServiceError::Cancelled)
            })
        });
    let mut job = job(candidates, service);
    job.config.spider_concurrency = 2;
    let (stop_tx, stop) = watch::channel(false);
    let outcome = bounded(job.run_spider_pass_until(stop, &stop_tx)).await;
    let expected = if panic {
        CrawlerRunError::SpiderTaskFailed
    } else {
        CrawlerRunError::SpiderCandidateLookupFailed
    };
    assert_eq!(outcome.into_result(), Err(expected));
    assert!(joined.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_keep_completed_expected_site_failure_nonfatal() {
    let mut candidates = candidates_for(CrawlerDomainId::new());
    candidates
        .expect_mark_crawl_failure()
        .once()
        .withf(|_, kind, count, next| {
            kind == "EmptyCrawl"
                && *count == 1
                && next_crawl_at_is_about(*next, CRAWL_RETRY_COOLDOWN)
        })
        .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
    let mut service = MockSpiderService::new();
    service
        .expect_run_until()
        .once()
        .returning(|_, _, _, _, _| {
            Box::pin(async {
                Err(SpiderServiceError::EmptyCrawl {
                    crawl_root_url: "redacted".into(),
                })
            })
        });
    let job = job(candidates, service);
    let (stop_tx, stop) = watch::channel(false);
    let outcome = bounded(job.run_spider_pass_until(stop, &stop_tx)).await;
    assert!(!outcome.admission_stopped());
    assert_eq!(outcome.into_result(), Ok(()));
    assert!(!*stop_tx.borrow());
}

#[rstest::rstest]
#[case::provider(GeneratedPatternFailure::Provider)]
#[case::generated_regex(GeneratedPatternFailure::Regex)]
#[case::no_products(GeneratedPatternFailure::NoProducts)]
#[tokio::test]
async fn should_cool_down_generated_pattern_errors_without_stopping(
    #[case] failure: GeneratedPatternFailure,
    #[values(false, true)] metadata_fails: bool,
) {
    let mut candidates = candidates_for(CrawlerDomainId::new());
    candidates.expect_reset_crawl_failure().never();
    candidates
        .expect_mark_crawl_failure()
        .once()
        .withf(|_, kind, count, next| {
            kind == "spider_run_error"
                && *count == 1
                && next_crawl_at_is_about(
                    *next,
                    durable_retry_cooldown_for(NetworkErrorKind::Unknown),
                )
        })
        .returning(move |_, _, _, _| {
            Box::pin(async move {
                if metadata_fails {
                    Err(sqlx::Error::PoolClosed)
                } else {
                    Ok(())
                }
            })
        });
    let mut service = MockSpiderService::new();
    service
        .expect_run_until()
        .once()
        .returning(move |_, _, _, _, _| Box::pin(async move { Err(failure.error()) }));
    let job = job(candidates, service);
    let (stop_tx, stop) = watch::channel(false);
    let outcome = bounded(job.run_spider_pass_until(stop, &stop_tx)).await;
    assert_eq!(outcome.admission_stopped(), metadata_fails);
    assert_eq!(
        outcome.into_result(),
        if metadata_fails {
            Err(CrawlerRunError::SpiderFailureMarkFailed)
        } else {
            Ok(())
        }
    );
    assert_eq!(*stop_tx.borrow(), metadata_fails);
}

#[tokio::test]
async fn should_reject_invalid_persisted_pattern_even_after_successful_cooldown_write() {
    let mut candidates = candidates_for(CrawlerDomainId::new());
    candidates.expect_reset_crawl_failure().never();
    candidates
        .expect_mark_crawl_failure()
        .once()
        .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
    let mut service = MockSpiderService::new();
    service
        .expect_run_until()
        .once()
        .returning(|_, _, _, _, _| {
            Box::pin(async {
                Err(SpiderServiceError::UrlPattern(
                    UrlPatternServiceError::Regex(regex::Error::Syntax(
                        "fake invalid persisted pattern".into(),
                    )),
                ))
            })
        });
    let job = job(candidates, service);
    let (stop_tx, stop) = watch::channel(false);
    let outcome = bounded(job.run_spider_pass_until(stop, &stop_tx)).await;
    assert_eq!(
        outcome.into_result(),
        Err(CrawlerRunError::SpiderServiceFailed)
    );
    assert!(*stop_tx.borrow());
}

#[rstest::rstest]
#[case::service_error(false)]
#[case::service_panic(true)]
#[tokio::test]
async fn should_stop_siblings_cancel_lookup_and_join_after_child_failure(#[case] panic: bool) {
    let slow_id = CrawlerDomainId::new();
    let failed_id = CrawlerDomainId::new();
    let lookup = PendingWork::default();
    let mut candidates = MockSpiderCandidateService::new();
    let pending = lookup.clone();
    candidates
        .expect_get_candidates()
        .times(2)
        .returning(move |_, excluded| {
            let first = excluded.is_empty();
            let pending = pending.clone();
            Box::pin(async move {
                if first {
                    Ok(vec![spider_candidate(slow_id), spider_candidate(failed_id)])
                } else {
                    pending.wait().await
                }
            })
        });
    candidates
        .expect_mark_crawl_failure()
        .times(usize::from(!panic))
        .returning(|_, _, _, _| Box::pin(async { Ok(()) }));
    let entered = Arc::new(Notify::new());
    let fail = Arc::new(Notify::new());
    let stopping = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let joined = Arc::new(AtomicBool::new(false));
    let mut service = MockSpiderService::new();
    let (started, trigger, stopped, finish, finished) = (
        entered.clone(),
        fail.clone(),
        stopping.clone(),
        release.clone(),
        joined.clone(),
    );
    service
        .expect_run_until()
        .times(2)
        .returning(move |_, id, _, _, mut stop| {
            let slow = *id == slow_id;
            let (started, trigger, stopped, finish, finished) = (
                started.clone(),
                trigger.clone(),
                stopped.clone(),
                finish.clone(),
                finished.clone(),
            );
            Box::pin(async move {
                if slow {
                    started.notify_one();
                    wait_for_stop(&mut stop).await;
                    stopped.notify_one();
                    finish.notified().await;
                    finished.store(true, Ordering::SeqCst);
                    Err(SpiderServiceError::Cancelled)
                } else {
                    trigger.notified().await;
                    assert!(!panic, "injected service panic");
                    Err(SpiderServiceError::Database(sqlx::Error::PoolClosed))
                }
            })
        });
    let mut job = job(candidates, service);
    job.config.spider_concurrency = 3;
    let returned = AtomicBool::new(false);
    let (stop_tx, stop) = watch::channel(false);
    let (outcome, ()) = bounded(async {
        tokio::join!(
            async {
                let outcome = job.run_spider_pass_until(stop, &stop_tx).await;
                returned.store(true, Ordering::SeqCst);
                outcome
            },
            async {
                lookup.entered.notified().await;
                entered.notified().await;
                fail.notify_one();
                stopping.notified().await;
                assert!(*stop_tx.borrow(), "failure must request sibling stop");
                assert!(
                    !returned.load(Ordering::SeqCst),
                    "retain pending service join"
                );
                release.notify_one();
            }
        )
    })
    .await;
    let expected = if panic {
        CrawlerRunError::SpiderTaskFailed
    } else {
        CrawlerRunError::SpiderServiceFailed
    };
    assert_eq!(outcome.into_result(), Err(expected));
    assert!(lookup.dropped.load(Ordering::SeqCst));
    assert!(joined.load(Ordering::SeqCst));
    assert!(DomainLock::try_acquire(&job.lock_manager, slow_id).is_some());
}

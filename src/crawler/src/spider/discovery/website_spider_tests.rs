use super::*;
use futures::{future::pending, poll};
use tokio::time::{advance, timeout};

const TEST_BUDGET: Duration = Duration::from_secs(60);

fn page(path: &str) -> CrawledPage {
    CrawledPage {
        url: CrawledUrl::new(Url::parse(&format!("https://example.com{path}")).unwrap()),
    }
}

fn library_page(path: &str) -> Page {
    spider::page::build(
        &format!("https://example.com{path}"),
        spider::utils::PageResponse {
            content: Some(b"<html><body><h1>Catalog</h1><p>Synthetic antique catalog page for a channel-only test.</p></body></html>".to_vec()),
            status_code: spider::reqwest::StatusCode::OK,
            ..Default::default()
        },
    )
}

fn library_crawl<P>(
    broadcast_capacity: usize,
    producer: impl FnOnce(broadcast::Sender<Page>, oneshot::Sender<(CrawlStatus, WebsiteMetaInfo)>) -> P,
) -> SpiderCrawl
where
    P: Future<Output = Result<(), CrawlIncompleteError>> + Send + 'static,
{
    let (pages_tx, pages_rx) = broadcast::channel(broadcast_capacity);
    let (status_tx, status_rx) = oneshot::channel();
    let (tx, rx) = mpsc::channel(1);
    let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
    SpiderCrawl::spawn_owned(
        rx,
        diagnostics_rx,
        producer(pages_tx, status_tx),
        forward_pages(
            pages_rx,
            status_rx,
            tx,
            diagnostics_tx,
            Url::parse("https://example.com/").unwrap(),
            Bloom::new_for_fp_rate(100, 0.001).unwrap(),
        ),
        TEST_BUDGET,
    )
}

fn assert_incomplete<T: std::fmt::Debug>(
    result: Result<T, SpiderDiscoveryError>,
    expected: CrawlIncompleteError,
) {
    assert!(
        matches!(&result, Err(SpiderDiscoveryError::Incomplete(actual)) if *actual == expected),
        "expected {expected:?}, got {result:?}"
    );
}

async fn assert_dropped(lifetime: oneshot::Receiver<()>) {
    // The sender is a lifetime probe, never a producer of a message.
    assert!(
        timeout(Duration::from_secs(1), lifetime)
            .await
            .unwrap()
            .is_err()
    );
}

struct BlockedCrawl {
    crawl: SpiderCrawl,
    blocked: oneshot::Receiver<()>,
    producer_dropped: oneshot::Receiver<()>,
    forwarder_dropped: oneshot::Receiver<()>,
}

fn blocked_crawl(budget: Duration) -> BlockedCrawl {
    let (tx, rx) = mpsc::channel(1);
    let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
    let (producer_lifetime, producer_dropped) = oneshot::channel();
    let (forwarder_lifetime, forwarder_dropped) = oneshot::channel();
    let (blocked_tx, blocked) = oneshot::channel();
    let crawl = SpiderCrawl::spawn_owned(
        rx,
        diagnostics_rx,
        async move {
            let _lifetime = producer_lifetime;
            pending().await
        },
        async move {
            let _lifetime = forwarder_lifetime;
            let _diagnostics = diagnostics_tx;
            tx.send(page("/first")).await.unwrap();
            let send = tx.send(page("/blocked"));
            tokio::pin!(send);
            assert!(poll!(send.as_mut()).is_pending());
            blocked_tx.send(()).unwrap();
            send.await
                .map_err(|_| CrawlIncompleteError::PageDeliveryClosed)
        },
        budget,
    );
    BlockedCrawl {
        crawl,
        blocked,
        producer_dropped,
        forwarder_dropped,
    }
}

#[tokio::test(start_paused = true)]
async fn should_deliver_pages_and_diagnostics_when_both_tasks_complete() {
    let mut crawl = library_crawl(4, |pages, status| async move {
        pages.send(library_page("/")).unwrap();
        pages.send(library_page("/product/1")).unwrap();
        pages.send(library_page("/product/1")).unwrap();
        status
            .send((CrawlStatus::Idle, WebsiteMetaInfo::default()))
            .unwrap();
        Ok(())
    });
    let mut delivered = Vec::new();
    while let Some(page) = crawl.recv().await.unwrap() {
        delivered.push(page.url.to_string());
    }
    assert_eq!(
        delivered,
        ["https://example.com/", "https://example.com/product/1"]
    );
    assert!(crawl.tasks.is_empty());
    let diagnostics = crawl.completion().await.unwrap();
    assert_eq!(diagnostics.http_status, Some(200));
    assert_eq!(
        diagnostics.final_url.as_deref(),
        Some("https://example.com/")
    );
    assert_eq!(diagnostics.failure_kind, None);
    assert!(crawl.recv().await.unwrap().is_none());
    assert_eq!(
        crawl.cancel_and_join().await.unwrap().http_status,
        Some(200)
    );
}

#[tokio::test(start_paused = true)]
async fn should_apply_producer_status_when_page_delivery_finishes() {
    let mut crawl = library_crawl(1, |pages, status| async move {
        pages.send(library_page("/")).unwrap();
        status
            .send((CrawlStatus::RateLimited, WebsiteMetaInfo::default()))
            .unwrap();
        Ok(())
    });
    assert!(crawl.recv().await.unwrap().is_some());
    assert!(crawl.recv().await.unwrap().is_none());
    assert_eq!(
        crawl.completion().await.unwrap().failure_kind,
        Some(CrawlFailureKind::RateLimited)
    );
}

#[tokio::test(start_paused = true)]
async fn should_wait_for_both_joins_when_pages_and_diagnostics_already_closed() {
    let (tx, rx) = mpsc::channel(1);
    let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
    let (producer_finish, producer_gate) = oneshot::channel();
    let (forwarder_finish, forwarder_gate) = oneshot::channel();
    let (published_tx, published) = oneshot::channel();
    let mut crawl = SpiderCrawl::spawn_owned(
        rx,
        diagnostics_rx,
        async move {
            producer_gate.await.unwrap();
            Ok(())
        },
        async move {
            diagnostics_tx.send(CrawlDiagnostics::default()).unwrap();
            drop(tx);
            published_tx.send(()).unwrap();
            forwarder_gate.await.unwrap();
            Ok(())
        },
        TEST_BUDGET,
    );
    published.await.unwrap();
    assert!(poll!(Box::pin(crawl.recv())).is_pending());
    forwarder_finish.send(()).unwrap();
    tokio::task::yield_now().await;
    assert!(poll!(Box::pin(crawl.completion())).is_pending());
    producer_finish.send(()).unwrap();
    assert!(crawl.recv().await.unwrap().is_none());
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_report_lag_and_stop_producer_even_after_delivering_pages() {
    let (continue_tx, continue_rx) = oneshot::channel();
    let (producer_lifetime, producer_dropped) = oneshot::channel();
    let mut crawl = library_crawl(1, |pages, status| async move {
        let _lifetime = producer_lifetime;
        let _status = status;
        pages.send(library_page("/first")).unwrap();
        continue_rx.await.unwrap();
        // No yield between sends: a one-slot broadcast must lose one message.
        pages.send(library_page("/lost")).unwrap();
        pages.send(library_page("/last")).unwrap();
        pending().await
    });
    assert_eq!(
        crawl.recv().await.unwrap().unwrap().url.to_string(),
        "https://example.com/first"
    );
    continue_tx.send(()).unwrap();
    // Peer shutdown must not depend on the consumer polling again.
    assert_dropped(producer_dropped).await;
    let expected = CrawlIncompleteError::BroadcastLagged { skipped: 1 };
    assert_incomplete(crawl.recv().await, expected);
    assert_incomplete(crawl.completion().await, expected);
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_report_missing_status_when_producer_drops_status_sender() {
    let mut crawl = library_crawl(1, |pages, status| async move {
        drop(pages);
        drop(status);
        Ok(())
    });
    assert_incomplete(crawl.recv().await, CrawlIncompleteError::MissingStatus);
    assert_incomplete(
        crawl.completion().await,
        CrawlIncompleteError::MissingStatus,
    );
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_report_missing_diagnostics_when_tasks_finish_without_delivery() {
    let mut crawl = SpiderCrawl::fixture(Vec::new(), None);
    assert_incomplete(crawl.recv().await, CrawlIncompleteError::MissingDiagnostics);
    assert_incomplete(
        crawl.completion().await,
        CrawlIncompleteError::MissingDiagnostics,
    );
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_report_redacted_failure_when_owned_producer_panics() {
    let mut crawl = library_crawl(1, |pages, status| async move {
        let _pages = pages;
        let _status = status;
        // Inject unwinding without invoking the process-global panic hook.
        std::panic::resume_unwind(Box::new("private producer payload"))
    });
    let error = crawl.recv().await.unwrap_err();
    assert!(!format!("{error} {error:?} {error:#?}").contains("private producer payload"));
    assert_incomplete::<()>(Err(error), CrawlIncompleteError::TaskPanicked);
    assert_incomplete(crawl.completion().await, CrawlIncompleteError::TaskPanicked);
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_stop_owned_producer_when_forwarder_panics_without_consumer_polling() {
    let (tx, rx) = mpsc::channel(1);
    let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
    let (producer_lifetime, producer_dropped) = oneshot::channel();
    let mut crawl = SpiderCrawl::spawn_owned(
        rx,
        diagnostics_rx,
        async move {
            let _lifetime = producer_lifetime;
            pending().await
        },
        async move {
            let _tx = tx;
            let _diagnostics = diagnostics_tx;
            std::panic::resume_unwind(Box::new("private forwarder payload"))
        },
        TEST_BUDGET,
    );
    assert_dropped(producer_dropped).await;
    assert_incomplete(crawl.recv().await, CrawlIncompleteError::TaskPanicked);
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_stop_idle_forwarder_and_producer_when_page_consumer_closes() {
    let (producer_lifetime, producer_dropped) = oneshot::channel();
    let mut crawl = library_crawl(1, |pages, status| async move {
        let _lifetime = producer_lifetime;
        let _pages = pages;
        let _status = status;
        pending().await
    });
    crawl.pages.close();
    assert_dropped(producer_dropped).await;
    assert_incomplete(
        crawl.completion().await,
        CrawlIncompleteError::PageDeliveryClosed,
    );
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_report_sender_failure_when_diagnostics_consumer_closes() {
    let mut crawl = library_crawl(1, |pages, status| async move {
        drop(pages);
        status
            .send((CrawlStatus::Idle, WebsiteMetaInfo::default()))
            .unwrap();
        Ok(())
    });
    crawl.diagnostics.close();
    assert_incomplete(
        crawl.completion().await,
        CrawlIncompleteError::DiagnosticsDeliveryClosed,
    );
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_cancel_and_join_both_tasks_when_page_delivery_is_blocked() {
    let BlockedCrawl {
        mut crawl,
        blocked,
        producer_dropped,
        forwarder_dropped,
    } = blocked_crawl(TEST_BUDGET);
    blocked.await.unwrap();
    assert_incomplete(
        crawl.cancel_and_join().await,
        CrawlIncompleteError::Cancelled,
    );
    assert!(crawl.tasks.is_empty());
    assert_dropped(producer_dropped).await;
    assert_dropped(forwarder_dropped).await;
    assert_incomplete(crawl.recv().await, CrawlIncompleteError::Cancelled);
    assert_incomplete(
        crawl.cancel_and_join().await,
        CrawlIncompleteError::Cancelled,
    );
}

#[tokio::test(start_paused = true)]
async fn should_abort_both_owned_tasks_on_drop_when_page_delivery_is_blocked() {
    let BlockedCrawl {
        crawl,
        blocked,
        producer_dropped,
        forwarder_dropped,
    } = blocked_crawl(TEST_BUDGET);
    blocked.await.unwrap();
    drop(crawl);
    assert_dropped(producer_dropped).await;
    assert_dropped(forwarder_dropped).await;
}

#[tokio::test(start_paused = true)]
async fn should_keep_task_ownership_when_pending_recv_or_completion_is_cancelled() {
    let (producer_lifetime, producer_dropped) = oneshot::channel();
    let mut crawl = library_crawl(1, |pages, status| async move {
        let _lifetime = producer_lifetime;
        let _pages = pages;
        let _status = status;
        pending().await
    });
    assert!(poll!(Box::pin(crawl.recv())).is_pending());
    assert!(poll!(Box::pin(crawl.completion())).is_pending());
    assert_incomplete(
        crawl.cancel_and_join().await,
        CrawlIncompleteError::Cancelled,
    );
    assert!(crawl.tasks.is_empty());
    assert_dropped(producer_dropped).await;
}

#[tokio::test(start_paused = true)]
async fn should_fail_at_deadline_and_stop_tasks_when_consumer_does_not_drain() {
    let BlockedCrawl {
        mut crawl,
        blocked,
        producer_dropped,
        forwarder_dropped,
    } = blocked_crawl(Duration::from_secs(5));
    blocked.await.unwrap();
    advance(Duration::from_secs(4)).await;
    assert!(poll!(Box::pin(crawl.completion())).is_pending());
    assert!(crawl.failure.borrow().is_none());
    advance(Duration::from_secs(1)).await;
    assert_dropped(producer_dropped).await;
    assert_dropped(forwarder_dropped).await;
    assert_incomplete(crawl.recv().await, CrawlIncompleteError::DeadlineExceeded);
    assert_incomplete(
        crawl.completion().await,
        CrawlIncompleteError::DeadlineExceeded,
    );
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_reject_ready_success_when_crawl_budget_is_already_exhausted() {
    let (tx, rx) = mpsc::channel(1);
    let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
    let mut crawl = SpiderCrawl::spawn_owned(
        rx,
        diagnostics_rx,
        async { Ok(()) },
        async move {
            drop(tx);
            diagnostics_tx.send(CrawlDiagnostics::default()).unwrap();
            Ok(())
        },
        Duration::ZERO,
    );
    assert_incomplete(
        crawl.completion().await,
        CrawlIncompleteError::DeadlineExceeded,
    );
    assert!(crawl.tasks.is_empty());
}

#[tokio::test(start_paused = true)]
async fn should_propagate_missing_diagnostics_without_service_classification_or_checkpoint() {
    use crate::spider::classification::url_metadata_repository::MockUrlMetadataRepository;
    use crate::spider::classification::url_pattern_service::MockUrlPatternService;
    use crate::spider::service::spider_service::{
        SpiderService, SpiderServiceConfig, SpiderServiceError, SpiderServiceImpl,
    };
    use std::sync::Arc;

    let mut spider = MockSpider::new();
    spider
        .expect_crawl()
        .times(1)
        .returning(|_| Box::pin(async { Ok(SpiderCrawl::fixture(Vec::new(), None)) }));
    let mut patterns = MockUrlPatternService::new();
    patterns
        .expect_load_pattern_for_domain()
        .times(1)
        .returning(|_, _| Box::pin(async { Ok(None) }));
    patterns.expect_classify_and_save().times(0);
    patterns.expect_mark_as_crawled().times(0);
    let mut urls = MockUrlMetadataRepository::new();
    urls.expect_upsert_links_batch().times(0);
    let service = SpiderServiceImpl::new(
        SpiderServiceConfig::default(),
        Box::new(spider),
        Box::new(patterns),
        Arc::new(urls),
    );
    let result = service
        .run(
            &listing_source_core::ListingSourceId::new(),
            &crate::CrawlerDomainId::new(),
            "https://example.com/",
            20,
        )
        .await;
    assert!(matches!(
        result,
        Err(SpiderServiceError::Discovery(
            SpiderDiscoveryError::Incomplete(CrawlIncompleteError::MissingDiagnostics)
        ))
    ));
}

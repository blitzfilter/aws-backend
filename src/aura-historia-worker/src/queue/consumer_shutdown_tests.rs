use super::*;
use std::sync::mpsc;
use tokio::sync::Notify;

struct CancellationGate {
    entered: Notify,
    release: Mutex<mpsc::Receiver<()>>,
    destroyed: AtomicBool,
}

fn cancellation_gate() -> (Arc<CancellationGate>, mpsc::Sender<()>) {
    let (release, wait) = mpsc::channel();
    (
        Arc::new(CancellationGate {
            entered: Notify::new(),
            release: Mutex::new(wait),
            destroyed: AtomicBool::new(false),
        }),
        release,
    )
}

struct BlockedDrop(Arc<CancellationGate>);
impl Drop for BlockedDrop {
    fn drop(&mut self) {
        self.0.entered.notify_one();
        // Synchronous destructor barrier. Timeout/disconnection also releases on test failure.
        let _released = self
            .0
            .release
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3));
        self.0.destroyed.store(true, Ordering::SeqCst);
    }
}

struct BlockedNormalization {
    started: Notify,
    calls: AtomicUsize,
    cancellation: Arc<CancellationGate>,
}

#[async_trait::async_trait]
impl NormalizeProductListingRawRevisionUseCase for BlockedNormalization {
    async fn execute(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
    {
        let _drop = BlockedDrop(self.cancellation.clone());
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(matches!(
            command.mode,
            NormalizeProductListingRawRevisionMode::Reconcile
        ));
        self.started.notify_one();
        std::future::pending().await
    }
}

// Owned only by the receiver; its destruction proves the pending/held receipt owner was joined.
struct CancellationTransport {
    inner: Arc<FakeTransport>,
    _drop: BlockedDrop,
}

#[async_trait::async_trait]
impl Transport for CancellationTransport {
    async fn send(&self, body: &str) -> Result<(), QueueError> {
        self.inner.send(body).await
    }
    async fn receive(&self) -> Result<Option<Message>, QueueError> {
        self.inner.receive().await
    }
    async fn visibility(&self, receipt: &str, seconds: i32) -> Result<(), QueueError> {
        self.inner.visibility(receipt, seconds).await
    }
    async fn delete(&self, receipt: &str) -> Result<(), QueueError> {
        self.inner.delete(receipt).await
    }
    async fn probe(&self) -> Result<(), QueueError> {
        self.inner.probe().await
    }
}

#[rstest::rstest]
#[case::pending_receive_first(false, false)]
#[case::pending_reconcile_first(false, true)]
#[case::held_receive_first(true, false)]
#[case::held_reconcile_first(true, true)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn should_join_active_reconciliation_and_receive_before_cancelled_cleanup_completes(
    #[case] held: bool,
    #[case] reconcile_first: bool,
) {
    let (reconcile_gate, release_reconcile) = cancellation_gate();
    let (receive_gate, release_receive) = cancellation_gate();
    let fake = Arc::new(FakeTransport::default());
    fake.receive_hangs.store(true, Ordering::SeqCst);
    fake.push(&wire::encode(&raw_job()).unwrap());
    let mut queue = scoped_queue(fake.clone(), WorkerScope::ProductListingRawNormalization);
    queue.transport = Arc::new(CancellationTransport {
        inner: fake.clone(),
        _drop: BlockedDrop(receive_gate.clone()),
    });
    let control = RuntimeControl::new(false);
    let receiver = WorkerQueueReceiver::sqs(queue, control.clone());
    let use_case = Arc::new(BlockedNormalization {
        started: Notify::new(),
        calls: AtomicUsize::new(0),
        cancellation: reconcile_gate.clone(),
    });
    let (_shutdown, shutdown_rx) = watch::channel(false);
    let consumer = tokio::spawn(
        crate::product_listing_raw_normalization::consume_product_listing_raw_normalization_queue(
            receiver,
            use_case.clone(),
            shutdown_rx,
        ),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        use_case.started.notified().await;
        if held {
            fake.receive_release.notify_one();
        }
        while fake.count(Call::Receive) != 1 || fake.receive_dropped.load(Ordering::SeqCst) != held
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    control.shutdown();
    consumer.abort();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), consumer)
            .await
            .unwrap()
            .is_err()
    );
    let cleanup_control = control.clone();
    let mut cleanup = tokio::spawn(async move { cleanup_control.join_cancelled_tasks().await });
    tokio::time::timeout(Duration::from_secs(1), async {
        reconcile_gate.entered.notified().await;
        receive_gate.entered.notified().await;
    })
    .await
    .unwrap();
    assert!(!reconcile_gate.destroyed.load(Ordering::SeqCst));
    assert!(!receive_gate.destroyed.load(Ordering::SeqCst));
    let (first, last) = if reconcile_first {
        (release_reconcile, release_receive)
    } else {
        (release_receive, release_reconcile)
    };
    first.send(()).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut cleanup)
            .await
            .is_err(),
        "cleanup must retain both joins, regardless of destruction order"
    );
    last.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), cleanup)
        .await
        .unwrap()
        .unwrap();
    assert!(reconcile_gate.destroyed.load(Ordering::SeqCst));
    assert!(receive_gate.destroyed.load(Ordering::SeqCst));
    assert!(!control.live());
    assert!(!control.ready());
    yield_tasks().await;
    assert_eq!(1, use_case.calls.load(Ordering::SeqCst));
    assert_eq!(
        vec![Call::Receive],
        fake.calls(),
        "no ack, visibility release or new receive"
    );
}

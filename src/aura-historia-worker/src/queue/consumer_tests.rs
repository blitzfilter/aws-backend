use super::super::{
    API_TIMEOUT, Message, QueueError, RECEIVE_TIMEOUT, SqsQueue, SqsQueueConfig, Transport, bounded,
};
use super::{
    JobOutcome, RuntimeControl, WorkerQueueReceiver, deferred_seconds, execute_owned, retry_delay,
    settle,
};
use crate::{
    WorkerScope,
    jobs::{
        DomainJob, DomainJobPayload, IdempotencyKey, NotificationDeliveryCreatedJob, OrderingKey,
        ProductListingRawRevisionJob, WorkerQueue,
    },
    wire,
};
use notification_core::notification_delivery_id::NotificationDeliveryId;
use product_listing_service::ports::{ProductListingRawRevisionId, ProductListingRawStreamId};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionError,
    NormalizeProductListingRawRevisionMode, NormalizeProductListingRawRevisionResult,
    NormalizeProductListingRawRevisionUseCase,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use time::OffsetDateTime;
use tokio::{
    sync::{oneshot, watch},
    task::JoinSet,
    time::Instant,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Receive,
    Visibility(i32),
    Delete,
    Probe,
    Send,
}
#[derive(Default)]
struct FakeTransport {
    calls: Mutex<Vec<Call>>,
    messages: Mutex<VecDeque<Message>>,
    bodies: Mutex<Vec<String>>,
    delete_failures: AtomicUsize,
    visibility_fails: AtomicBool,
    visibility_failures: AtomicUsize,
    probe_fails: AtomicBool,
    send_fails_at: AtomicUsize,
    send_hangs: AtomicBool,
    ambiguous_send_at: AtomicUsize,
    visibility_hangs: AtomicBool,
    delete_hangs: AtomicBool,
    receive_fails: AtomicBool,
    receive_panics: AtomicBool,
    receive_hangs: AtomicBool,
    receive_dropped: Arc<AtomicBool>,
    probe_hangs: AtomicBool,
    receive_release: tokio::sync::Notify,
    visibility_release: tokio::sync::Notify,
    visibility_completed: AtomicUsize,
    send_release: tokio::sync::Notify,
}
impl FakeTransport {
    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn count(&self, call: Call) -> usize {
        self.calls()
            .iter()
            .filter(|actual| **actual == call)
            .count()
    }
    fn push(&self, body: &str) {
        self.messages.lock().unwrap().push_back(Message {
            body: Some(body.into()),
            receipt: "private-receipt".into(),
            receive_count: 1,
            sent_timestamp_ms: 0,
            first_received_timestamp_ms: 0,
        });
    }
}
#[async_trait::async_trait]
impl Transport for FakeTransport {
    async fn send(&self, body: &str) -> Result<(), QueueError> {
        self.record(Call::Send);
        if self.send_hangs.load(Ordering::SeqCst) {
            self.send_release.notified().await;
        }
        if self.count(Call::Send) == self.send_fails_at.load(Ordering::SeqCst) {
            return Err(QueueError::Unavailable);
        }
        self.bodies.lock().unwrap().push(body.into());
        if self.count(Call::Send) == self.ambiguous_send_at.load(Ordering::SeqCst) {
            return Err(QueueError::Timeout);
        }
        Ok(())
    }
    async fn receive(&self) -> Result<Option<Message>, QueueError> {
        let _guard = DropFlag(self.receive_dropped.clone());
        self.record(Call::Receive);
        assert!(
            !self.receive_panics.load(Ordering::SeqCst),
            "simulated receive panic"
        );
        if self.receive_fails.load(Ordering::SeqCst) {
            return Err(QueueError::Unavailable);
        }
        let message = self.messages.lock().unwrap().pop_front();
        // Model SQS reserving a receipt before the response reaches the caller.
        if self.receive_hangs.load(Ordering::SeqCst) {
            self.receive_release.notified().await;
        }
        if message.is_none() {
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
        Ok(message)
    }
    async fn visibility(&self, receipt: &str, seconds: i32) -> Result<(), QueueError> {
        assert_eq!("private-receipt", receipt);
        self.record(Call::Visibility(seconds));
        if self.visibility_hangs.load(Ordering::SeqCst) {
            self.visibility_release.notified().await;
        }
        if self.visibility_fails.load(Ordering::SeqCst)
            || self
                .visibility_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            Err(QueueError::Unavailable)
        } else {
            self.visibility_completed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    async fn delete(&self, receipt: &str) -> Result<(), QueueError> {
        assert_eq!("private-receipt", receipt);
        self.record(Call::Delete);
        if self.delete_hangs.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self
            .delete_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            Err(QueueError::Unavailable)
        } else {
            Ok(())
        }
    }
    async fn probe(&self) -> Result<(), QueueError> {
        self.record(Call::Probe);
        if self.probe_hangs.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.probe_fails.load(Ordering::SeqCst) {
            Err(QueueError::Unavailable)
        } else {
            Ok(())
        }
    }
}
const UUID_V7_BASE: u128 = 0x0190_0000_0000_7000_8000_0000_0000_0000;
const NOTIFICATION_DELIVERY_UUID_1: &str = "01900000-0000-7000-8000-000000000001";
const NOTIFICATION_DELIVERY_UUID_2: &str = "01900000-0000-7000-8000-000000000002";
const NOTIFICATION_DELIVERY_TYPE_ID_1: &str = "nd_01j0000000e008000000000001";

fn uuid_v7(low_bits: u64) -> uuid::Uuid {
    uuid::Uuid::from_u128(UUID_V7_BASE | u128::from(low_bits))
}

fn notification_delivery_id(low_bits: u64) -> NotificationDeliveryId {
    NotificationDeliveryId::try_from(uuid_v7(low_bits))
        .unwrap_or_else(|error| panic!("valid notification delivery UUIDv7 fixture: {error}"))
}

fn raw_stream_id(low_bits: u64) -> ProductListingRawStreamId {
    ProductListingRawStreamId::try_from(uuid_v7(low_bits))
        .unwrap_or_else(|error| panic!("valid raw stream UUIDv7 fixture: {error}"))
}

fn raw_revision_id(low_bits: u64) -> ProductListingRawRevisionId {
    ProductListingRawRevisionId::try_from(uuid_v7(low_bits))
        .unwrap_or_else(|error| panic!("valid raw revision UUIDv7 fixture: {error}"))
}

fn job() -> DomainJob {
    let notification_delivery_id = notification_delivery_id(1);
    DomainJob {
        target_queue: WorkerQueue::NotificationDelivery,
        idempotency_key: IdempotencyKey::new(format!(
            "notification-delivery:{notification_delivery_id}"
        )),
        ordering_key: OrderingKey::new(format!("notification-delivery:{notification_delivery_id}")),
        payload: DomainJobPayload::NotificationDeliveryCreated(NotificationDeliveryCreatedJob {
            notification_delivery_id,
        }),
    }
}
fn message() -> Message {
    Message {
        body: Some(wire::encode(&job()).unwrap()),
        receipt: "private-receipt".into(),
        receive_count: 1,
        sent_timestamp_ms: 0,
        first_received_timestamp_ms: 0,
    }
}

#[test]
fn should_encode_schema_v2_notification_typeid_fixture() {
    let job = job();
    let encoded = wire::encode(&job).unwrap();
    let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();

    assert_eq!(serde_json::json!(2), value["schema_version"]);
    assert_eq!(
        serde_json::json!(NOTIFICATION_DELIVERY_TYPE_ID_1),
        value["payload"]["notification_delivery_id"]
    );
    assert_eq!(
        serde_json::json!(format!(
            "notification-delivery:{NOTIFICATION_DELIVERY_TYPE_ID_1}"
        )),
        value["idempotency_key"]
    );
    assert_eq!(
        Some(26),
        NOTIFICATION_DELIVERY_TYPE_ID_1
            .strip_prefix("nd_")
            .map(str::len)
    );
}
fn queue(fake: Arc<FakeTransport>) -> SqsQueue {
    scoped_queue(fake, WorkerScope::NotificationDelivery)
}
fn scoped_queue(fake: Arc<FakeTransport>, scope: WorkerScope) -> SqsQueue {
    SqsQueue {
        config: SqsQueueConfig::new(
            scope,
            format!(
                "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{}-test",
                scope.as_str()
            )
            .parse()
            .unwrap(),
            "eu-central-1".into(),
            "test".into(),
            None,
        )
        .unwrap(),
        transport: fake,
    }
}
async fn yield_tasks() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

struct GatedNormalization {
    commands: tokio::sync::mpsc::UnboundedSender<NormalizeProductListingRawRevisionMode>,
    reconciliation_release: tokio::sync::Notify,
    cdc_release: tokio::sync::Notify,
    active: tokio::sync::Mutex<()>,
}
#[async_trait::async_trait]
impl NormalizeProductListingRawRevisionUseCase for GatedNormalization {
    async fn execute(
        &self,
        command: NormalizeProductListingRawRevisionCommand,
    ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
    {
        let _active = self
            .active
            .try_lock()
            .expect("at most one normalization executes");
        let cdc = matches!(
            command.mode,
            NormalizeProductListingRawRevisionMode::RawRevision { .. }
        );
        self.commands.send(command.mode).unwrap();
        if cdc {
            self.cdc_release.notified().await;
        } else {
            self.reconciliation_release.notified().await;
        }
        Ok(NormalizeProductListingRawRevisionResult::default())
    }
}
fn gated_normalization() -> (
    Arc<GatedNormalization>,
    tokio::sync::mpsc::UnboundedReceiver<NormalizeProductListingRawRevisionMode>,
) {
    let (commands, receiver) = tokio::sync::mpsc::unbounded_channel();
    (
        Arc::new(GatedNormalization {
            commands,
            reconciliation_release: Default::default(),
            cdc_release: Default::default(),
            active: Default::default(),
        }),
        receiver,
    )
}
fn raw_job() -> DomainJob {
    let product_listing_raw_stream_id = raw_stream_id(1);
    let product_listing_raw_revision_id = raw_revision_id(2);
    DomainJob {
        target_queue: WorkerQueue::ProductListingRawNormalization,
        idempotency_key: IdempotencyKey::new(format!(
            "product-listing-raw-revision:{product_listing_raw_revision_id}"
        )),
        ordering_key: OrderingKey::new(format!(
            "product-listing-raw-stream:{product_listing_raw_stream_id}"
        )),
        payload: DomainJobPayload::ProductListingRawRevision(ProductListingRawRevisionJob {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision: 1,
        }),
    }
}

#[tokio::test(start_paused = true)]
async fn should_keep_poll_and_heartbeat_reserved_receipt_during_reconciliation_then_alternate() {
    use crate::product_listing_raw_normalization::consume_product_listing_raw_normalization_queue;
    let scope = WorkerScope::ProductListingRawNormalization;
    let fake = Arc::new(FakeTransport::default());
    fake.receive_hangs.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        fake.push(&wire::encode(&raw_job()).unwrap());
    }
    let control = RuntimeControl::new(false);
    let receiver = WorkerQueueReceiver::sqs(scoped_queue(fake.clone(), scope), control.clone());
    let (use_case, mut commands) = gated_normalization();
    let (shutdown, shutdown_rx) = watch::channel(false);
    let consumer = tokio::spawn(consume_product_listing_raw_normalization_queue(
        receiver,
        use_case.clone(),
        shutdown_rx,
    ));
    assert!(matches!(
        commands.recv().await,
        Some(NormalizeProductListingRawRevisionMode::Reconcile)
    ));
    tokio::time::advance(Duration::from_secs(3)).await;
    yield_tasks().await;
    assert_eq!(1, fake.count(Call::Receive));
    assert!(!fake.receive_dropped.load(Ordering::SeqCst));
    fake.receive_release.notify_one();
    yield_tasks().await;
    assert!(fake.receive_dropped.load(Ordering::SeqCst));
    tokio::time::advance(Duration::from_secs(30)).await;
    yield_tasks().await;
    assert_eq!(vec![Call::Receive, Call::Visibility(300)], fake.calls());
    assert!(commands.try_recv().is_err());
    use_case.reconciliation_release.notify_one();
    assert!(matches!(
        commands.recv().await,
        Some(NormalizeProductListingRawRevisionMode::RawRevision { revision: 1, .. })
    ));
    assert_eq!(1, fake.count(Call::Receive));
    tokio::time::advance(Duration::from_secs(30)).await;
    yield_tasks().await;
    assert_eq!(2, fake.count(Call::Visibility(300)));
    assert_eq!(1, fake.count(Call::Receive));
    assert_eq!(0, fake.count(Call::Delete));
    use_case.cdc_release.notify_one();
    assert!(matches!(
        commands.recv().await,
        Some(NormalizeProductListingRawRevisionMode::Reconcile)
    ));
    assert_eq!(1, fake.count(Call::Delete));
    shutdown.send_replace(true);
    use_case.reconciliation_release.notify_one();
    consumer.await.unwrap();
    assert!(!control.live());
    assert!(!control.ready());
    assert!(commands.try_recv().is_err());
    assert!(fake.count(Call::Receive) <= 2);
    assert_eq!(0, fake.count(Call::Visibility(0)));
    let calls = fake.calls();
    tokio::time::advance(Duration::from_secs(600)).await;
    yield_tasks().await;
    assert_eq!(calls, fake.calls());
}

#[tokio::test(start_paused = true)]
async fn should_stop_normalizer_and_mark_health_failed_when_owned_receive_panics() {
    let fake = Arc::new(FakeTransport::default());
    fake.receive_panics.store(true, Ordering::SeqCst);
    let control = RuntimeControl::new(false);
    let receiver = WorkerQueueReceiver::sqs(
        scoped_queue(fake.clone(), WorkerScope::ProductListingRawNormalization),
        control.clone(),
    );
    let (use_case, _commands) = gated_normalization();
    use_case.reconciliation_release.notify_one();
    let (_shutdown, shutdown_rx) = watch::channel(false);
    crate::product_listing_raw_normalization::consume_product_listing_raw_normalization_queue(
        receiver,
        use_case,
        shutdown_rx,
    )
    .await;
    assert!(!control.live());
    assert!(!control.ready());
    assert_eq!(vec![Call::Receive], fake.calls());
}

#[tokio::test(start_paused = true)]
async fn should_keep_one_receive_owned_across_cancelled_readiness_waits() {
    let fake = Arc::new(FakeTransport::default());
    fake.receive_hangs.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        fake.push(&wire::encode(&job()).unwrap());
    }
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), RuntimeControl::new(false));
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let mut polling = receiver.into_polling();
    yield_tasks().await;
    for _ in 0..3 {
        assert!(
            tokio::time::timeout(Duration::from_secs(1), polling.ready())
                .await
                .is_err()
        );
        assert_eq!(1, fake.count(Call::Receive));
        assert_eq!(1, fake.messages.lock().unwrap().len());
        assert!(!fake.receive_dropped.load(Ordering::SeqCst));
    }
    fake.receive_release.notify_one();
    yield_tasks().await;
    // A due timer may win even when receipt readiness is already queued.
    tokio::select! {
        biased;
        () = std::future::ready(()) => {}
        () = polling.ready() => panic!("timer has priority"),
    }
    polling.ready().await;
    let (mut receiver, delivery) = polling.take().await.unwrap();
    assert!(polling.task.is_empty());
    receiver
        .process(delivery.unwrap(), |_| async {
            JobOutcome::Complete("done")
        })
        .await;
    assert_eq!(vec![Call::Receive, Call::Delete], fake.calls());
    assert_eq!(1, fake.messages.lock().unwrap().len());
    polling.stop().await;
}

#[tokio::test(start_paused = true)]
async fn should_join_held_heartbeat_before_handoff_execution_and_delete_without_prefetch() {
    let fake = Arc::new(FakeTransport::default());
    fake.visibility_hangs.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        fake.push(&wire::encode(&job()).unwrap());
    }
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), RuntimeControl::new(false));
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let mut polling = receiver.into_polling();
    polling.ready().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    yield_tasks().await;
    assert_eq!(vec![Call::Receive, Call::Visibility(360)], fake.calls());
    let observed = fake.clone();
    let handoff = tokio::spawn(async move {
        let (mut receiver, delivery) = polling.take().await.unwrap();
        assert!(polling.task.is_empty());
        receiver
            .process(delivery.unwrap(), move |_| async move {
                assert_eq!(1, observed.visibility_completed.load(Ordering::SeqCst));
                assert_eq!(1, observed.count(Call::Receive));
                JobOutcome::Complete("done")
            })
            .await;
    });
    yield_tasks().await;
    assert!(!handoff.is_finished());
    assert_eq!(0, fake.count(Call::Delete));
    assert_eq!(0, fake.visibility_completed.load(Ordering::SeqCst));
    fake.visibility_release.notify_one();
    handoff.await.unwrap();
    let calls = fake.calls();
    assert_eq!(
        vec![Call::Receive, Call::Visibility(360), Call::Delete],
        calls
    );
    assert_eq!(1, fake.messages.lock().unwrap().len());
    tokio::time::advance(Duration::from_secs(600)).await;
    yield_tasks().await;
    assert_eq!(calls, fake.calls());
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test(start_paused = true)]
async fn should_never_execute_or_delete_after_held_heartbeat_failure(#[case] hangs: bool) {
    let fake = Arc::new(FakeTransport::default());
    fake.visibility_hangs.store(hangs, Ordering::SeqCst);
    fake.visibility_fails.store(!hangs, Ordering::SeqCst);
    fake.push(&wire::encode(&job()).unwrap());
    let control = RuntimeControl::new(false);
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let mut polling = receiver.into_polling();
    polling.ready().await;
    tokio::time::advance(Duration::from_secs(30)).await;
    yield_tasks().await;
    if hangs {
        tokio::time::advance(API_TIMEOUT).await;
        yield_tasks().await;
    }
    assert!(control.live());
    assert!(!control.ready());
    let (mut receiver, delivery) = polling.take().await.unwrap();
    let delivery = delivery.unwrap();
    assert_eq!(
        Some("held_receipt_heartbeat_failed"),
        delivery.lease_failure
    );
    fake.visibility_hangs.store(false, Ordering::SeqCst);
    fake.visibility_fails.store(false, Ordering::SeqCst);
    let executed = Arc::new(AtomicBool::new(false));
    let observed = executed.clone();
    receiver
        .process(delivery, move |_| async move {
            observed.store(true, Ordering::SeqCst);
            JobOutcome::Complete("must_not_execute")
        })
        .await;
    assert!(!executed.load(Ordering::SeqCst));
    assert_eq!(0, fake.count(Call::Delete));
    assert_eq!(0, fake.count(Call::Visibility(0)));
    let calls = fake.calls();
    tokio::time::advance(Duration::from_secs(600)).await;
    yield_tasks().await;
    assert_eq!(calls, fake.calls());
    polling.stop().await;
}

#[tokio::test(start_paused = true)]
async fn should_bound_held_receipt_lifetime_without_immortal_heartbeat_or_execution() {
    let fake = Arc::new(FakeTransport::default());
    fake.push(&wire::encode(&job()).unwrap());
    let control = RuntimeControl::new(false);
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let mut polling = receiver.into_polling();
    polling.ready().await;
    for _ in 0..8 {
        tokio::time::advance(Duration::from_secs(30)).await;
        yield_tasks().await;
    }
    assert_eq!(8, fake.count(Call::Visibility(360)));
    tokio::time::advance(API_TIMEOUT).await;
    yield_tasks().await;
    assert!(!control.ready());
    let (mut receiver, delivery) = polling.take().await.unwrap();
    let delivery = delivery.unwrap();
    assert_eq!(Some("held_receipt_deadline"), delivery.lease_failure);
    let executed = Arc::new(AtomicBool::new(false));
    let observed = executed.clone();
    receiver
        .process(delivery, move |_| async move {
            observed.store(true, Ordering::SeqCst);
            JobOutcome::Complete("must_not_execute")
        })
        .await;
    assert!(!executed.load(Ordering::SeqCst));
    assert_eq!(0, fake.count(Call::Delete));
    let calls = fake.calls();
    tokio::time::advance(Duration::from_secs(600)).await;
    yield_tasks().await;
    assert_eq!(calls, fake.calls());
    polling.stop().await;
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test(start_paused = true)]
async fn should_join_pending_or_held_receive_on_shutdown_without_releasing_or_deleting(
    #[case] held: bool,
) {
    let fake = Arc::new(FakeTransport::default());
    fake.receive_hangs.store(!held, Ordering::SeqCst);
    fake.visibility_hangs.store(held, Ordering::SeqCst);
    fake.push(&wire::encode(&job()).unwrap());
    let control = RuntimeControl::new(false);
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let mut polling = receiver.into_polling();
    yield_tasks().await;
    if held {
        polling.ready().await;
        tokio::time::advance(Duration::from_secs(30)).await;
        yield_tasks().await;
        assert_eq!(1, fake.count(Call::Visibility(360)));
    }
    let started = Instant::now();
    polling.stop().await;
    assert_eq!(
        if held { API_TIMEOUT } else { Duration::ZERO },
        started.elapsed()
    );
    assert!(polling.task.is_empty());
    assert!(fake.receive_dropped.load(Ordering::SeqCst));
    assert_eq!(0, fake.count(Call::Delete));
    assert_eq!(0, fake.count(Call::Visibility(0)));
    assert_eq!(1, fake.count(Call::Receive));
    drop(guard);
    assert!(!control.live());
    assert!(!control.ready());
    let calls = fake.calls();
    tokio::time::advance(Duration::from_secs(600)).await;
    yield_tasks().await;
    assert_eq!(calls, fake.calls());
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test(start_paused = true)]
async fn should_abort_pending_receive_children_when_scheduler_owner_is_dropped(#[case] held: bool) {
    let fake = Arc::new(FakeTransport::default());
    fake.receive_hangs.store(!held, Ordering::SeqCst);
    fake.push(&wire::encode(&job()).unwrap());
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), RuntimeControl::new(false));
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let mut polling = receiver.into_polling();
    yield_tasks().await;
    if held {
        polling.ready().await;
        tokio::time::advance(Duration::from_secs(30)).await;
        yield_tasks().await;
        assert_eq!(1, fake.count(Call::Visibility(360)));
    }
    drop(polling);
    yield_tasks().await;
    assert!(fake.receive_dropped.load(Ordering::SeqCst));
    let calls = fake.calls();
    tokio::time::advance(Duration::from_secs(600)).await;
    yield_tasks().await;
    assert_eq!(calls, fake.calls());
    assert_eq!(0, fake.count(Call::Delete));
    assert_eq!(0, fake.count(Call::Visibility(0)));
}

#[rstest::rstest]
#[case(60, 20)]
#[case(300, 30)]
#[case(360, 30)]
#[tokio::test(start_paused = true)]
async fn should_cap_heartbeat_period_at_thirty_seconds(
    #[case] visibility: u64,
    #[case] period: u64,
) {
    let fake = FakeTransport::default();
    let message = message();
    let outcome = execute_owned(
        Some(&fake),
        Some(&message),
        Duration::from_secs(visibility),
        Duration::from_secs(240),
        async move {
            tokio::time::sleep(Duration::from_secs(period * 2 + 1)).await;
            JobOutcome::Complete("done")
        },
    )
    .await;
    settle(&fake, &message, outcome).await.unwrap();
    assert_eq!(
        vec![
            Call::Visibility(visibility as i32),
            Call::Visibility(visibility as i32),
            Call::Delete
        ],
        fake.calls()
    );
    tokio::time::advance(Duration::from_secs(visibility * 2)).await;
    assert_eq!(3, fake.calls().len());
}

#[tokio::test(start_paused = true)]
async fn should_heartbeat_then_stop_heartbeat_before_delete() {
    let fake = FakeTransport::default();
    let message = message();
    let outcome = execute_owned(
        Some(&fake),
        Some(&message),
        Duration::from_secs(60),
        Duration::from_secs(90),
        async {
            tokio::time::sleep(Duration::from_secs(45)).await;
            JobOutcome::Complete("done")
        },
    )
    .await;
    settle(&fake, &message, outcome).await.unwrap();
    assert_eq!(
        vec![Call::Visibility(60), Call::Visibility(60), Call::Delete],
        fake.calls()
    );
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(3, fake.calls().len());
}
#[tokio::test(start_paused = true)]
async fn should_stop_heartbeat_before_retry_visibility_and_never_delete_errors() {
    for outcome in [
        JobOutcome::Retry("failed"),
        JobOutcome::Invalid("poison"),
        JobOutcome::DependencyUnavailable("outage"),
        JobOutcome::TransportUnavailable("sqs_outage"),
        JobOutcome::RetryAfter(OffsetDateTime::now_utc() + time::Duration::minutes(5)),
    ] {
        let fake = FakeTransport::default();
        let message = message();
        let result = execute_owned(
            Some(&fake),
            Some(&message),
            Duration::from_secs(60),
            Duration::from_secs(90),
            async move {
                tokio::time::sleep(Duration::from_secs(21)).await;
                outcome
            },
        )
        .await;
        settle(&fake, &message, result).await.unwrap();
        assert_eq!(0, fake.count(Call::Delete));
        assert_eq!(Call::Visibility(60), fake.calls()[0]);
        let count = fake.calls().len();
        tokio::time::advance(Duration::from_secs(120)).await;
        assert_eq!(count, fake.calls().len());
    }
}
#[tokio::test(start_paused = true)]
async fn should_bound_delete_retries_without_rerunning_handler() {
    for failures in [2, 100] {
        let fake = Arc::new(FakeTransport::default());
        fake.delete_failures.store(failures, Ordering::SeqCst);
        fake.push(&wire::encode(&job()).unwrap());
        let mut receiver =
            WorkerQueueReceiver::sqs(queue(fake.clone()), RuntimeControl::new(false));
        let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let captured = calls.clone();
        let delivery = receiver.recv().await.unwrap();
        receiver
            .process(delivery, move |_| async move {
                captured.fetch_add(1, Ordering::SeqCst);
                JobOutcome::Complete("done")
            })
            .await;
        assert_eq!(1, calls.load(Ordering::SeqCst));
        assert_eq!(3, fake.count(Call::Delete));
        assert_eq!(1, fake.count(Call::Receive));
    }
}
#[tokio::test(start_paused = true)]
async fn should_not_dispatch_or_delete_poison_unknown_schema_wrong_scope_or_invalid_keys() {
    let good = serde_json::from_str::<serde_json::Value>(&wire::encode(&job()).unwrap()).unwrap();
    for (field, bad) in [
        ("schema_version", serde_json::json!(1)),
        ("scope", serde_json::json!("product-translation")),
        ("idempotency_key", serde_json::json!("forged")),
        ("job_type", serde_json::json!("UNKNOWN")),
        (
            "notification_delivery_id",
            serde_json::json!("usr_01j0000000e008000000000001"),
        ),
    ] {
        let fake = Arc::new(FakeTransport::default());
        let mut value = good.clone();
        if field == "notification_delivery_id" {
            value["payload"][field] = bad;
        } else {
            value[field] = bad;
        }
        fake.push(&value.to_string());
        let mut receiver =
            WorkerQueueReceiver::sqs(queue(fake.clone()), RuntimeControl::new(false));
        let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
        let delivery = receiver.recv().await.unwrap();
        let invocations = Arc::new(AtomicUsize::new(0));
        let observed = invocations.clone();
        receiver
            .process(delivery, move |_| {
                observed.fetch_add(1, Ordering::SeqCst);
                async { JobOutcome::Complete("must_not_execute") }
            })
            .await;
        assert_eq!(
            0,
            invocations.load(Ordering::SeqCst),
            "poison field: {field}"
        );
        assert_eq!(0, fake.count(Call::Delete));
        assert!(matches!(
            fake.calls().last(),
            Some(Call::Visibility(30..=45))
        ));
    }
}
struct DropFlag(Arc<AtomicBool>);
impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
#[tokio::test(start_paused = true)]
async fn should_cancel_and_join_handler_on_timeout_and_heartbeat_failure_without_ack() {
    for heartbeat_fails in [false, true] {
        let fake = FakeTransport::default();
        fake.visibility_fails
            .store(heartbeat_fails, Ordering::SeqCst);
        let message = message();
        let dropped = Arc::new(AtomicBool::new(false));
        let captured = dropped.clone();
        let outcome = execute_owned(
            Some(&fake),
            Some(&message),
            Duration::from_secs(60),
            Duration::from_secs(45),
            async move {
                let _guard = DropFlag(captured);
                std::future::pending::<JobOutcome>().await
            },
        )
        .await;
        assert!(dropped.load(Ordering::SeqCst));
        assert!(!matches!(outcome, JobOutcome::Complete(_)));
        assert_eq!(0, fake.count(Call::Delete));
    }
}
#[tokio::test]
async fn should_contain_handler_panic_and_abort_children_when_owner_is_cancelled() {
    let fake = Arc::new(FakeTransport::default());
    let result = execute_owned(
        Some(fake.as_ref()),
        Some(&message()),
        Duration::from_secs(60),
        Duration::from_secs(45),
        async { panic!("simulated panic") },
    )
    .await;
    assert!(matches!(result, JobOutcome::Retry(_)));
    assert_eq!(0, fake.count(Call::Delete));
    let dropped = Arc::new(AtomicBool::new(false));
    let captured = dropped.clone();
    let transport = fake.clone();
    let (started, wait) = oneshot::channel();
    let owner = tokio::spawn(async move {
        execute_owned(
            Some(transport.as_ref()),
            Some(&message()),
            Duration::from_secs(60),
            Duration::from_secs(45),
            async move {
                let _guard = DropFlag(captured);
                started.send(()).unwrap();
                std::future::pending::<JobOutcome>().await
            },
        )
        .await
    });
    wait.await.unwrap();
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    yield_tasks().await;
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(0, fake.count(Call::Delete));
}
#[tokio::test]
async fn should_reserve_capacity_before_receive_and_drain_active_job_on_shutdown() {
    let fake = Arc::new(FakeTransport::default());
    let body = wire::encode(&job()).unwrap();
    fake.push(&body);
    fake.push(&body);
    let control = RuntimeControl::new(false);
    let receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let captured = release.clone();
    let consumer = tokio::spawn(receiver.run(WorkerScope::NotificationDelivery, move |_| {
        let release = captured.clone();
        let started = started.clone();
        async move {
            started.send(()).unwrap();
            release.notified().await;
            JobOutcome::Complete("done")
        }
    }));
    starts.recv().await.unwrap();
    assert!(control.live());
    assert!(control.ready());
    assert_eq!(1, fake.count(Call::Receive));
    control.shutdown();
    yield_tasks().await;
    assert!(!consumer.is_finished());
    release.notify_one();
    consumer.await.unwrap();
    assert_eq!(1, fake.count(Call::Receive));
    assert_eq!(1, fake.count(Call::Delete));
    assert!(!control.live());
}
#[tokio::test]
async fn should_mark_dead_consumer_unhealthy_when_cancelled() {
    let fake = Arc::new(FakeTransport::default());
    let control = RuntimeControl::new(false);
    let receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let consumer = tokio::spawn(receiver.run(WorkerScope::NotificationDelivery, |_| async {
        JobOutcome::Complete("done")
    }));
    yield_tasks().await;
    assert!(control.live());
    consumer.abort();
    let _cancelled = consumer.await;
    assert!(!control.live());
    assert!(!control.ready());
}
#[tokio::test(start_paused = true)]
async fn should_pause_outage_then_allow_only_one_half_open_recovery_probe() {
    let fake = Arc::new(FakeTransport::default());
    let body = wire::encode(&job()).unwrap();
    fake.push(&body);
    fake.push(&body);
    let control = RuntimeControl::new(false);
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let delivery = receiver.recv().await.unwrap();
    receiver
        .process(delivery, |_| async {
            JobOutcome::DependencyUnavailable("database_down")
        })
        .await;
    assert!(control.live());
    assert!(!control.ready());
    let poll = tokio::spawn(async move {
        let delivery = receiver.recv().await.unwrap();
        (receiver, delivery)
    });
    tokio::time::advance(Duration::from_secs(29)).await;
    yield_tasks().await;
    assert_eq!(1, fake.count(Call::Receive));
    assert_eq!(0, fake.count(Call::Probe));
    tokio::time::advance(Duration::from_secs(17)).await;
    let (mut receiver, delivery) = poll.await.unwrap();
    assert_eq!(1, fake.count(Call::Probe));
    assert_eq!(2, fake.count(Call::Receive));
    receiver
        .process(delivery, |_| async { JobOutcome::Complete("recovered") })
        .await;
    assert!(control.ready());
    assert_eq!(0, receiver.circuit_failures);
}
#[tokio::test(start_paused = true)]
async fn should_not_receive_more_messages_while_recovery_probe_fails() {
    let fake = Arc::new(FakeTransport::default());
    fake.probe_fails.store(true, Ordering::SeqCst);
    let control = RuntimeControl::new(false);
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    receiver.pause();
    let consumer = tokio::spawn(async move { receiver.recv().await.is_none() });
    tokio::time::advance(Duration::from_secs(46)).await;
    yield_tasks().await;
    assert_eq!(0, fake.count(Call::Receive));
    assert_eq!(1, fake.count(Call::Probe));
    control.shutdown();
    assert!(consumer.await.unwrap());
}
#[tokio::test(start_paused = true)]
async fn should_allow_twenty_second_long_poll_with_bounded_sdk_wrapper() {
    let fake = FakeTransport::default();
    let started = Instant::now();
    assert!(
        bounded(fake.receive(), RECEIVE_TIMEOUT)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(Duration::from_secs(20), started.elapsed());
}
#[test]
fn should_bound_retry_jitter_and_round_lease_expiry_up() {
    for attempt in 0..100 {
        for sample in [0, 7, u64::MAX] {
            let delay = retry_delay(attempt, sample);
            assert!((Duration::from_secs(30)..=Duration::from_secs(900)).contains(&delay));
        }
    }
    assert_eq!(Duration::from_secs(30), retry_delay(1, 0));
    assert_eq!(Duration::from_secs(60), retry_delay(2, 0));
    assert_eq!(Duration::from_secs(900), retry_delay(100, 9));
    let now = OffsetDateTime::UNIX_EPOCH;
    assert_eq!(
        301,
        deferred_seconds(
            now + time::Duration::seconds(300) + time::Duration::nanoseconds(1),
            now
        )
    );
    assert_eq!(1, deferred_seconds(now - time::Duration::seconds(1), now));
}

#[tokio::test(start_paused = true)]
async fn should_release_execution_slot_without_sleeping_for_job_retry() {
    let fake = Arc::new(FakeTransport::default());
    fake.push(&wire::encode(&job()).unwrap());
    fake.push(&wire::encode(&job()).unwrap());
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), RuntimeControl::new(false));
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let delivery = receiver.recv().await.unwrap();
    let started = Instant::now();
    receiver
        .process(delivery, |_| async { JobOutcome::Retry("source_missing") })
        .await;
    assert!(receiver.recv().await.is_some());
    assert_eq!(Duration::ZERO, started.elapsed());
    assert_eq!(2, fake.count(Call::Receive));
    assert_eq!(0, fake.count(Call::Delete));
}

#[tokio::test(start_paused = true)]
async fn should_bound_hung_heartbeat_delete_and_failed_retry_visibility_without_loss() {
    let fake = Arc::new(FakeTransport::default());
    fake.visibility_hangs.store(true, Ordering::SeqCst);
    let dropped = Arc::new(AtomicBool::new(false));
    let captured = dropped.clone();
    let started = Instant::now();
    let outcome = execute_owned(
        Some(fake.as_ref()),
        Some(&message()),
        Duration::from_secs(60),
        Duration::from_secs(45),
        async move {
            let _guard = DropFlag(captured);
            std::future::pending::<JobOutcome>().await
        },
    )
    .await;
    assert_eq!(
        JobOutcome::TransportUnavailable("heartbeat_failed"),
        outcome
    );
    assert_eq!(Duration::from_secs(25), started.elapsed());
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(
        Err(QueueError::Timeout),
        settle(fake.as_ref(), &message(), outcome).await
    );
    assert_eq!(0, fake.count(Call::Delete));
    fake.visibility_hangs.store(false, Ordering::SeqCst);
    fake.visibility_fails.store(true, Ordering::SeqCst);
    assert_eq!(
        Err(QueueError::Unavailable),
        settle(fake.as_ref(), &message(), JobOutcome::Retry("failed")).await
    );
    fake.delete_hangs.store(true, Ordering::SeqCst);
    let started = Instant::now();
    assert_eq!(
        Err(QueueError::Timeout),
        settle(fake.as_ref(), &message(), JobOutcome::Complete("committed")).await
    );
    assert_eq!(Duration::from_secs(18), started.elapsed());
    assert_eq!(3, fake.count(Call::Delete));
    let calls = fake.calls();
    tokio::time::advance(Duration::from_secs(1200)).await;
    assert_eq!(calls, fake.calls());
}

#[tokio::test(start_paused = true)]
async fn should_not_close_dependency_circuit_for_poison_retries_or_active_claims() {
    for outcome in [
        JobOutcome::Invalid("poison"),
        JobOutcome::Retry("missing_source"),
        JobOutcome::RetryAfter(OffsetDateTime::now_utc() + time::Duration::minutes(5)),
    ] {
        let fake = Arc::new(FakeTransport::default());
        for _ in 0..3 {
            fake.push(&wire::encode(&job()).unwrap());
        }
        let control = RuntimeControl::new(false);
        let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
        let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
        let delivery = receiver.recv().await.unwrap();
        receiver
            .process(delivery, |_| async {
                JobOutcome::DependencyUnavailable("down")
            })
            .await;
        let delivery = receiver.recv().await.unwrap();
        receiver
            .process(delivery, move |_| async move { outcome })
            .await;
        assert!(control.live());
        assert!(!control.ready());
        assert_eq!(2, receiver.circuit_failures);
        let started = Instant::now();
        let delivery = receiver.recv().await.unwrap();
        assert!(started.elapsed() >= Duration::from_secs(60));
        assert_eq!(2, fake.count(Call::Probe));
        receiver
            .process(delivery, |_| async { JobOutcome::Complete("recovered") })
            .await;
        assert!(control.ready());
    }
}

#[rstest::rstest]
#[case::executing_transport_only(false, false)]
#[case::held_transport_only(true, false)]
#[case::executing_with_prior_service_outage(false, true)]
#[case::held_with_prior_service_outage(true, true)]
#[tokio::test(start_paused = true)]
async fn should_recover_on_empty_receive_after_heartbeat_failure_only_without_prior_service_outage(
    #[case] held: bool,
    #[case] prior_service_outage: bool,
) {
    let fake = Arc::new(FakeTransport::default());
    let control = RuntimeControl::new(false);
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    if prior_service_outage {
        fake.push(&wire::encode(&job()).unwrap());
        let delivery = receiver.recv().await.unwrap();
        receiver
            .process(delivery, |_| async {
                JobOutcome::DependencyUnavailable("database_down")
            })
            .await;
        assert!(receiver.service_probe_pending);
    }
    fake.push(&wire::encode(&job()).unwrap());
    fake.visibility_failures.store(1, Ordering::SeqCst);
    let (mut receiver, delivery) = if held {
        let mut polling = receiver.into_polling();
        polling.ready().await;
        tokio::time::advance(Duration::from_secs(30)).await;
        yield_tasks().await;
        let (receiver, delivery) = polling.take().await.unwrap();
        assert!(polling.task.is_empty());
        let delivery = delivery.unwrap();
        assert_eq!(
            Some("held_receipt_heartbeat_failed"),
            delivery.lease_failure
        );
        (receiver, delivery)
    } else {
        let delivery = receiver.recv().await.unwrap();
        (receiver, delivery)
    };
    let invocations = Arc::new(AtomicUsize::new(0));
    let observed = invocations.clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let drop_observed = dropped.clone();
    receiver
        .process(delivery, move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
            async move {
                let _guard = DropFlag(drop_observed);
                std::future::pending::<JobOutcome>().await
            }
        })
        .await;
    assert_eq!(usize::from(!held), invocations.load(Ordering::SeqCst));
    assert_eq!(!held, dropped.load(Ordering::SeqCst));
    assert_eq!(1, fake.count(Call::Visibility(360)));
    assert_eq!(0, fake.visibility_failures.load(Ordering::SeqCst));
    assert_eq!(
        1 + usize::from(prior_service_outage),
        fake.visibility_completed.load(Ordering::SeqCst)
    );
    assert!(control.live());
    assert!(!control.ready());
    assert_eq!(0, fake.count(Call::Delete));
    assert_eq!(0, fake.count(Call::Visibility(0)));
    let resume_at = receiver.pause_until.unwrap();
    let receives = fake.count(Call::Receive);
    let probes = fake.count(Call::Probe);
    let mut polling = receiver.into_polling();
    yield_tasks().await;
    let before_resume = resume_at - Instant::now() - Duration::from_millis(1);
    tokio::time::advance(before_resume).await;
    yield_tasks().await;
    assert_eq!(receives, fake.count(Call::Receive));
    assert_eq!(probes, fake.count(Call::Probe));
    assert!(!control.ready());
    tokio::time::advance(Duration::from_millis(1)).await;
    yield_tasks().await;
    assert_eq!(probes + 1, fake.count(Call::Probe));
    assert_eq!(receives + 1, fake.count(Call::Receive));
    assert!(!control.ready(), "attribute probe alone is not recovery");
    assert!(fake.messages.lock().unwrap().is_empty());
    tokio::time::advance(Duration::from_secs(20)).await;
    yield_tasks().await;
    assert_eq!(!prior_service_outage, control.ready());
    assert!(control.live());
    assert_eq!(0, fake.count(Call::Delete));
    assert_eq!(usize::from(!held), invocations.load(Ordering::SeqCst));
    control.shutdown();
    let (receiver, delivery) = polling.take().await.unwrap();
    assert!(delivery.is_none());
    assert_eq!(prior_service_outage, receiver.service_probe_pending);
    assert_eq!(prior_service_outage, receiver.circuit_failures > 0);
    polling.stop().await;
}

#[tokio::test(start_paused = true)]
async fn should_preserve_service_failure_completed_during_a_failing_heartbeat_call() {
    let fake = Arc::new(FakeTransport::default());
    fake.visibility_hangs.store(true, Ordering::SeqCst);
    fake.push(&wire::encode(&job()).unwrap());
    let control = RuntimeControl::new(false);
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    let delivery = receiver.recv().await.unwrap();
    let (started, started_rx) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let attempt = tokio::spawn(async move {
        receiver
            .process(delivery, |_| async move {
                started.send(()).unwrap();
                release_rx.await.unwrap();
                JobOutcome::DependencyUnavailable("database_down")
            })
            .await;
        receiver
    });
    started_rx.await.unwrap();
    tokio::time::advance(Duration::from_secs(30)).await;
    yield_tasks().await;
    assert_eq!(1, fake.count(Call::Visibility(360)));
    release.send(()).unwrap();
    yield_tasks().await;
    assert!(!attempt.is_finished());
    // The existing heartbeat remains hung; only the subsequent retry visibility succeeds.
    fake.visibility_hangs.store(false, Ordering::SeqCst);
    tokio::time::advance(API_TIMEOUT).await;
    let receiver = attempt.await.unwrap();
    assert!(receiver.service_probe_pending);
    assert!(receiver.pause_until.is_some());
    assert!(control.live());
    assert!(!control.ready());
    assert_eq!(0, fake.count(Call::Delete));
    assert_eq!(1, fake.visibility_completed.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn should_exponentially_bound_sustained_receive_outage_and_recover_empty_queue() {
    let fake = Arc::new(FakeTransport::default());
    fake.receive_fails.store(true, Ordering::SeqCst);
    let control = RuntimeControl::new(false);
    let receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let consumer = tokio::spawn(receiver.run(WorkerScope::NotificationDelivery, |_| async {
        JobOutcome::Complete("done")
    }));
    yield_tasks().await;
    assert_eq!(1, fake.count(Call::Receive));
    for _ in 0..360 {
        tokio::time::advance(Duration::from_secs(10)).await;
        yield_tasks().await;
    }
    assert!(control.live());
    assert!(!control.ready());
    assert!((6..=10).contains(&fake.count(Call::Receive)));
    assert_eq!(fake.count(Call::Receive) - 1, fake.count(Call::Probe));
    fake.receive_fails.store(false, Ordering::SeqCst);
    for _ in 0..100 {
        tokio::time::advance(Duration::from_secs(10)).await;
        yield_tasks().await;
        if control.ready() {
            break;
        }
    }
    assert!(control.ready());
    control.shutdown();
    consumer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn should_cancel_hung_recovery_probe_immediately_on_shutdown() {
    let fake = Arc::new(FakeTransport::default());
    fake.probe_hangs.store(true, Ordering::SeqCst);
    let control = RuntimeControl::new(false);
    let mut receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
    let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
    receiver.pause();
    let consumer = tokio::spawn(async move { receiver.recv().await.is_none() });
    yield_tasks().await;
    tokio::time::advance(Duration::from_secs(46)).await;
    yield_tasks().await;
    assert_eq!(1, fake.count(Call::Probe));
    let started = Instant::now();
    control.shutdown();
    assert!(consumer.await.unwrap());
    assert_eq!(Duration::ZERO, started.elapsed());
    assert_eq!(0, fake.count(Call::Receive));
}

#[tokio::test]
async fn should_mark_receiver_dead_on_receive_panic_but_contain_handler_panic() {
    for receive_panic in [false, true] {
        let fake = Arc::new(FakeTransport::default());
        fake.receive_panics.store(receive_panic, Ordering::SeqCst);
        fake.push(&wire::encode(&job()).unwrap());
        let control = RuntimeControl::new(false);
        let receiver = WorkerQueueReceiver::sqs(queue(fake.clone()), control.clone());
        let consumer = tokio::spawn(receiver.run(WorkerScope::NotificationDelivery, |_| async {
            panic!("simulated handler panic")
        }));
        yield_tasks().await;
        if receive_panic {
            assert!(consumer.await.unwrap_err().is_panic());
        } else {
            assert!(control.live());
            control.shutdown();
            consumer.await.unwrap();
        }
        assert!(!control.live());
        assert!(!control.ready());
        assert_eq!(0, fake.count(Call::Delete));
    }
}

#[tokio::test]
async fn should_retain_ambiguous_send_and_republish_entire_batch_with_same_keys() {
    let fake = Arc::new(FakeTransport::default());
    fake.ambiguous_send_at.store(2, Ordering::SeqCst);
    let fanout = crate::cdc::CdcFanout::for_scope(
        WorkerScope::NotificationDelivery,
        crate::cdc::WorkerQueueRegistry::new().with_sqs_queue(queue(fake.clone())),
    );
    let batch = batch(&[NOTIFICATION_DELIVERY_UUID_1, NOTIFICATION_DELIVERY_UUID_2]);
    assert!(fanout.ingest_batch(&batch).await.is_err());
    assert_eq!(2, fake.bodies.lock().unwrap().len());
    assert_eq!(2, fanout.ingest_batch(&batch).await.unwrap());
    let bodies = fake.bodies.lock().unwrap();
    assert_eq!(bodies[..2], bodies[2..]);
}

fn batch(ids: &[&str]) -> crate::cdc::CdcBatch {
    serde_json::from_value(serde_json::json!({"changes": ids.iter().map(|id| serde_json::json!({
        "schema":"public", "table":"notification_deliveries", "operation":"insert", "record":{"notification_delivery_id":id}
    })).collect::<Vec<_>>()})).unwrap()
}
#[tokio::test]
async fn should_drain_active_http_publication_before_joining_connections_on_shutdown() {
    let fake = Arc::new(FakeTransport::default());
    fake.send_hangs.store(true, Ordering::SeqCst);
    let (runtime, _receiver) =
        crate::WorkerRuntimeComposition::from_sqs_queue(queue(fake.clone())).into_parts();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut tasks = JoinSet::new();
    tasks.spawn(crate::serve_with_runtime(
        listener,
        runtime.clone(),
        std::future::pending::<()>(),
    ));
    let request = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!("http://{address}/cdc/sequin"))
            .body(
                serde_json::json!({"changes":[{"schema":"public","table":"notification_deliveries","operation":"insert","record":{"notification_delivery_id":NOTIFICATION_DELIVERY_UUID_1}}]}).to_string(),
            )
            .send()
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fake.count(Call::Send) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    runtime.shutdown();
    yield_tasks().await;
    assert!(tasks.try_join_next().is_none());
    fake.send_release.notify_one();
    assert_eq!(
        202,
        tokio::time::timeout(Duration::from_secs(2), request)
            .await
            .unwrap()
            .unwrap()
            .status()
            .as_u16()
    );
    tokio::time::timeout(Duration::from_secs(2), tasks.join_next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(1, fake.bodies.lock().unwrap().len());
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn should_reject_oversized_publication_before_calling_transport() {
    let fake = Arc::new(FakeTransport::default());
    let queue = queue(fake.clone());
    assert_eq!(
        Err(QueueError::MessageTooLarge),
        queue.publish(&"x".repeat(wire::MAX_JOB_BYTES + 1)).await
    );
    assert_eq!(0, fake.count(Call::Send));
}

#[derive(Clone, Default)]
struct RecordedEvents(Arc<Mutex<Vec<std::collections::BTreeMap<String, String>>>>);
struct EventFields(std::collections::BTreeMap<String, String>);
impl tracing::field::Visit for EventFields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
}
impl tracing::Subscriber for RecordedEvents {
    fn register_callsite(
        &self,
        _: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        // Other tests hit these callsites with different dispatchers. Filter per event.
        tracing::subscriber::Interest::sometimes()
    }
    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target().starts_with("aura_historia_worker::queue")
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = EventFields(Default::default());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
}

#[tokio::test(start_paused = true)]
async fn should_log_safe_receive_execution_and_publication_timings_including_cancellation() {
    use tracing::instrument::WithSubscriber;
    let events = RecordedEvents::default();
    // tracing-core's single-dispatcher fast path caches the registering thread's default.
    // Keep two dispatchers alive so parallel no-subscriber tests cannot disable our callsites.
    let _parallel_dispatch = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    async {
        let fake = Arc::new(FakeTransport::default());
        let queue = queue(fake.clone());
        let mut value =
            serde_json::from_str::<serde_json::Value>(&wire::encode(&job()).unwrap()).unwrap();
        value["private_future_metadata"] = serde_json::json!("sensitive-payload-marker");
        let body = value.to_string();
        queue.publish(&body).await.unwrap();
        fake.send_hangs.store(true, Ordering::SeqCst);
        assert!(
            tokio::time::timeout(Duration::from_millis(125), queue.publish(&body))
                .await
                .is_err()
        );
        fake.send_hangs.store(false, Ordering::SeqCst);
        fake.send_fails_at.store(3, Ordering::SeqCst);
        assert_eq!(Err(QueueError::Unavailable), queue.publish(&body).await);
        assert_eq!(
            Err(QueueError::MessageTooLarge),
            queue.publish(&"x".repeat(wire::MAX_JOB_BYTES + 1)).await
        );
        fake.push(&body);
        let mut receiver = WorkerQueueReceiver::sqs(queue, RuntimeControl::new(false));
        let _guard = receiver.start(WorkerScope::NotificationDelivery).unwrap();
        let mut polling = receiver.into_polling();
        polling.ready().await;
        let (mut receiver, delivery) = polling.take().await.unwrap();
        receiver
            .process(delivery.unwrap(), |_| async {
                tokio::time::sleep(Duration::from_millis(250)).await;
                JobOutcome::Complete("done")
            })
            .await;
    }
    .with_subscriber(events.clone())
    .await;
    let events = events.0.lock().unwrap();
    for outcome in [
        "published",
        "cancelled_acceptance_unknown",
        "failed_acceptance_unknown",
        "rejected_size",
    ] {
        let event = events
            .iter()
            .find(|event| event.get("outcome").map(String::as_str) == Some(outcome))
            .unwrap();
        assert!(event.contains_key("publication_duration_ms"));
        assert!(event.contains_key("encoded_bytes"));
        if outcome == "cancelled_acceptance_unknown" {
            assert_eq!(
                125.0,
                event["publication_duration_ms"].parse::<f64>().unwrap()
            );
        }
    }
    let received = events
        .iter()
        .find(|event| event.get("outcome").map(String::as_str) == Some("received"))
        .unwrap_or_else(|| {
            panic!(
                "missing received event; outcomes: {:?}",
                events
                    .iter()
                    .filter_map(|event| event.get("outcome"))
                    .collect::<Vec<_>>()
            )
        });
    for field in [
        "attempt",
        "sent_timestamp_ms",
        "first_received_timestamp_ms",
        "receive_duration_ms",
    ] {
        assert!(received.contains_key(field));
    }
    let executed = events
        .iter()
        .find(|event| event.get("outcome").map(String::as_str) == Some("done"))
        .unwrap();
    assert_eq!(
        250.0,
        executed["execution_duration_ms"].parse::<f64>().unwrap()
    );
    for event in events.iter() {
        for (field, value) in event {
            assert!(!field.contains("receipt"));
            assert!(!value.contains("private-receipt"));
            assert!(!value.contains("sensitive-payload-marker"));
        }
    }
}

#[tokio::test]
async fn should_prevalidate_entire_batch_before_any_sqs_publication() {
    let fake = Arc::new(FakeTransport::default());
    let fanout = crate::cdc::CdcFanout::for_scope(
        WorkerScope::NotificationDelivery,
        crate::cdc::WorkerQueueRegistry::new().with_sqs_queue(queue(fake.clone())),
    );
    assert!(
        fanout
            .ingest_batch(&batch(&[NOTIFICATION_DELIVERY_UUID_1, "bad-id"]))
            .await
            .is_err()
    );
    assert_eq!(0, fake.count(Call::Send));
    let too_many = batch(&vec![NOTIFICATION_DELIVERY_UUID_1; 101]);
    assert!(fanout.ingest_batch(&too_many).await.is_err());
    assert_eq!(0, fake.count(Call::Send));
}
#[tokio::test]
async fn should_retry_entire_batch_after_partial_sqs_publication_with_stable_keys() {
    let fake = Arc::new(FakeTransport::default());
    fake.send_fails_at.store(2, Ordering::SeqCst);
    let fanout = crate::cdc::CdcFanout::for_scope(
        WorkerScope::NotificationDelivery,
        crate::cdc::WorkerQueueRegistry::new().with_sqs_queue(queue(fake.clone())),
    );
    let batch = batch(&[NOTIFICATION_DELIVERY_UUID_1, NOTIFICATION_DELIVERY_UUID_2]);
    assert!(fanout.ingest_batch(&batch).await.is_err());
    assert_eq!(1, fake.bodies.lock().unwrap().len());
    assert_eq!(2, fanout.ingest_batch(&batch).await.unwrap());
    let bodies = fake.bodies.lock().unwrap();
    assert_eq!(bodies[0], bodies[1]);
    assert_ne!(bodies[1], bodies[2]);
}
#[tokio::test(start_paused = true)]
async fn should_bound_publication_when_sqs_hangs_and_never_ack() {
    let fake = Arc::new(FakeTransport::default());
    fake.send_hangs.store(true, Ordering::SeqCst);
    let fanout = crate::cdc::CdcFanout::for_scope(
        WorkerScope::NotificationDelivery,
        crate::cdc::WorkerQueueRegistry::new().with_sqs_queue(queue(fake.clone())),
    );
    let started = Instant::now();
    assert!(
        fanout
            .ingest_batch(&batch(&[NOTIFICATION_DELIVERY_UUID_1]))
            .await
            .is_err()
    );
    assert!(started.elapsed() <= crate::cdc::PUBLICATION_TIMEOUT);
    assert_eq!(0, fake.bodies.lock().unwrap().len());
}

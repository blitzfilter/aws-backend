#[cfg(test)]
#[path = "consumer_tests.rs"]
mod tests;

use super::{
    API_TIMEOUT, Message, RECEIVE_TIMEOUT, SqsQueue, Transport, bounded, config,
    tasks::{CancelledTasks, OwnedTasks},
};
use crate::{InMemoryQueueReceiver, WorkerScope, jobs::DomainJob, wire};
use std::{
    future::Future,
    hash::BuildHasher,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use time::OffsetDateTime;
use tokio::{
    sync::{oneshot, watch},
    time::{Instant, MissedTickBehavior},
};
use tracing::{info, instrument::WithSubscriber, warn};

#[derive(Debug, Clone)]
pub(crate) struct RuntimeControl(Arc<ControlState>);
#[derive(Debug)]
struct ControlState {
    live: AtomicBool,
    ready: AtomicBool,
    shutdown: watch::Sender<bool>,
    cancelled_tasks: CancelledTasks,
}
impl RuntimeControl {
    pub(crate) fn new(ready: bool) -> Self {
        let (shutdown, _) = watch::channel(false);
        Self(Arc::new(ControlState {
            live: AtomicBool::new(ready),
            ready: AtomicBool::new(ready),
            shutdown,
            cancelled_tasks: CancelledTasks::default(),
        }))
    }
    pub(crate) async fn join_cancelled_tasks(&self) {
        self.0.cancelled_tasks.join().await;
    }
    pub(crate) fn owned_tasks<T: Send + 'static>(&self) -> OwnedTasks<T> {
        OwnedTasks::new(self.0.cancelled_tasks.clone())
    }
    pub(crate) fn live(&self) -> bool {
        self.0.live.load(Ordering::Acquire) && !self.stopping()
    }
    pub(crate) fn ready(&self) -> bool {
        self.live() && self.0.ready.load(Ordering::Acquire)
    }
    pub(crate) fn stopping(&self) -> bool {
        *self.0.shutdown.borrow()
    }
    pub(crate) fn shutdown(&self) {
        self.0.shutdown.send_replace(true);
    }
    pub(crate) async fn cancelled(&self) {
        let mut shutdown = self.0.shutdown.subscribe();
        if *shutdown.borrow() {
            return;
        }
        let _closed = shutdown.wait_for(|shutdown| *shutdown).await;
    }
}

pub(crate) struct ConsumerGuard(RuntimeControl);
impl Drop for ConsumerGuard {
    fn drop(&mut self) {
        self.0.0.ready.store(false, Ordering::Release);
        self.0.0.live.store(false, Ordering::Release);
    }
}

/// A production consumer input, paired with the runtime's shutdown and health supervision.
/// The explicit `From<InMemoryQueueReceiver>` path exists for legacy test composition only.
pub struct WorkerQueueReceiver {
    source: Source,
    control: RuntimeControl,
    scope: Option<WorkerScope>,
    circuit_failures: u32,
    service_probe_pending: bool,
    pause_until: Option<Instant>,
}
enum Source {
    Sqs(Box<SqsQueue>),
    Memory(InMemoryQueueReceiver<DomainJob>),
}

impl From<InMemoryQueueReceiver<DomainJob>> for WorkerQueueReceiver {
    fn from(receiver: InMemoryQueueReceiver<DomainJob>) -> Self {
        Self {
            source: Source::Memory(receiver),
            control: RuntimeControl::new(true),
            scope: None,
            circuit_failures: 0,
            service_probe_pending: false,
            pause_until: None,
        }
    }
}

// Private transport disposition, not a service result. Only a durable terminal result is Complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobOutcome {
    Complete(&'static str),
    Retry(&'static str),
    RetryAfter(OffsetDateTime),
    Invalid(&'static str),
    DependencyUnavailable(&'static str),
    TransportUnavailable(&'static str),
}
impl JobOutcome {
    fn category(self) -> &'static str {
        match self {
            Self::Complete(category)
            | Self::Retry(category)
            | Self::Invalid(category)
            | Self::DependencyUnavailable(category)
            | Self::TransportUnavailable(category) => category,
            Self::RetryAfter(_) => "active_lease_deferred",
        }
    }
}

pub(crate) struct Delivery {
    job: Result<DomainJob, wire::WireError>,
    message: Option<Message>,
    lease_failure: Option<&'static str>,
}

/// One reserved receive slot across timer turns. Readiness does not transfer receipt ownership.
/// The owner keeps heartbeating until the scheduler selects CDC and explicitly joins the handoff.
pub(crate) struct PendingReceive {
    task: OwnedTasks<(WorkerQueueReceiver, Option<Delivery>)>,
    ready: oneshot::Receiver<()>,
    handoff: Option<oneshot::Sender<()>>,
    control: RuntimeControl,
}
impl PendingReceive {
    fn new(mut receiver: WorkerQueueReceiver) -> Self {
        let control = receiver.control();
        let (ready_tx, ready) = oneshot::channel();
        let (handoff, handoff_rx) = oneshot::channel();
        let mut task = OwnedTasks::new(control.0.cancelled_tasks.clone());
        task.spawn(
            async move {
                let mut delivery = receiver.recv().await;
                let _scheduler_closed = ready_tx.send(());
                if let Some(delivery) = &mut delivery {
                    receiver.hold_delivery(delivery, handoff_rx).await;
                }
                (receiver, delivery)
            }
            .with_current_subscriber(),
        );
        Self {
            task,
            ready,
            handoff: Some(handoff),
            control,
        }
    }

    pub(crate) async fn ready(&mut self) {
        // oneshot receive is cancellation-safe. A closed sender is observed by joining the task.
        let _owner_stopped = (&mut self.ready).await;
    }

    pub(crate) async fn take(&mut self) -> Option<(WorkerQueueReceiver, Option<Delivery>)> {
        if let Some(handoff) = self.handoff.take() {
            let _owner_stopped = handoff.send(());
        }
        match self.task.join_next().await {
            Some(Ok(result)) => Some(result),
            Some(Err(_)) => {
                warn!(
                    outcome = "receive_owner_failed",
                    "owned receive task failed; receipt not acknowledged"
                );
                None
            }
            None => None,
        }
    }

    pub(crate) async fn stop(&mut self) {
        self.control.shutdown();
        while let Some(result) = self.task.join_next().await {
            if result.is_err() {
                warn!(
                    outcome = "receive_owner_failed",
                    "owned receive task failed during shutdown"
                );
            }
        }
    }
}

impl WorkerQueueReceiver {
    pub(crate) fn sqs(queue: SqsQueue, control: RuntimeControl) -> Self {
        Self {
            scope: Some(queue.config.scope()),
            source: Source::Sqs(Box::new(queue)),
            control,
            circuit_failures: 0,
            service_probe_pending: false,
            pause_until: None,
        }
    }

    pub(crate) fn start(&mut self, scope: WorkerScope) -> Option<ConsumerGuard> {
        if self.scope.is_some_and(|actual| actual != scope) || self.control.stopping() {
            warn!(
                scope = scope.as_str(),
                outcome = "consumer_scope_mismatch",
                "consumer did not start"
            );
            return None;
        }
        self.scope = Some(scope);
        self.control.0.live.store(true, Ordering::Release);
        self.control.0.ready.store(true, Ordering::Release);
        Some(ConsumerGuard(self.control.clone()))
    }

    pub(crate) fn control(&self) -> RuntimeControl {
        self.control.clone()
    }

    pub(crate) fn into_polling(self) -> PendingReceive {
        PendingReceive::new(self)
    }

    async fn hold_delivery(&mut self, delivery: &mut Delivery, mut handoff: oneshot::Receiver<()>) {
        let Some(scope) = self.scope else {
            return;
        };
        let visibility = config::visibility(scope);
        let mut heartbeat = heartbeat_interval(visibility);
        // Only one bounded reconciliation turn may precede this ready CDC job.
        let deadline = Instant::now() + config::execution_budget(scope) + API_TIMEOUT;
        loop {
            tokio::select! {
                biased;
                _ = &mut handoff => return,
                () = self.control.cancelled() => return,
                () = tokio::time::sleep_until(deadline) => {
                    delivery.lease_failure = Some("held_receipt_deadline");
                    self.pause();
                    return;
                }
                _ = heartbeat.tick(), if delivery.message.is_some() => {
                    if let (Source::Sqs(queue), Some(message)) = (&self.source, &delivery.message)
                        && bounded(queue.transport.visibility(&message.receipt, visibility.as_secs() as i32), API_TIMEOUT).await.is_err()
                    {
                        delivery.lease_failure = Some("held_receipt_heartbeat_failed");
                        self.pause();
                        return;
                    }
                }
            }
        }
    }

    pub(crate) async fn run<F, Fut>(mut self, scope: WorkerScope, handler: F)
    where
        F: Fn(DomainJob) -> Fut,
        Fut: Future<Output = JobOutcome> + Send + 'static,
    {
        let Some(_guard) = self.start(scope) else {
            return;
        };
        while let Some(delivery) = self.recv().await {
            self.process(delivery, &handler).await;
        }
    }

    /// Reserve the single CDC slot before ReceiveMessage; never poll during execution/settlement.
    /// Not cancellation-safe: SQS may already have reserved a receipt. Timer schedulers must use
    /// `into_polling`; only shutdown may abandon a pending poll, without acknowledgment.
    pub(crate) async fn recv(&mut self) -> Option<Delivery> {
        loop {
            self.wait_for_receive_resume().await?;
            let receive_started = Instant::now();
            let received = match &mut self.source {
                Source::Memory(receiver) => {
                    return tokio::select! {
                        biased;
                        () = self.control.cancelled() => None,
                        job = receiver.recv() => job.map(|job| Delivery { job: Ok(job), message: None, lease_failure: None }),
                    };
                }
                Source::Sqs(queue) => tokio::select! {
                    biased;
                    () = self.control.cancelled() => return None,
                    result = bounded(queue.transport.receive(), RECEIVE_TIMEOUT) => result,
                },
            };
            match received {
                Ok(Some(message)) => {
                    info!(
                        scope = self.scope.map(WorkerScope::as_str),
                        attempt = message.receive_count,
                        sent_timestamp_ms = message.sent_timestamp_ms,
                        first_received_timestamp_ms = message.first_received_timestamp_ms,
                        receive_duration_ms = receive_started.elapsed().as_secs_f64() * 1000.0,
                        outcome = "received",
                        "worker received SQS job"
                    );
                    if !self.service_probe_pending {
                        self.recovered();
                    }
                    let scope = self.scope?;
                    let job = message
                        .body
                        .as_deref()
                        .ok_or(wire::WireError::Json)
                        .and_then(|body| wire::decode(body, scope));
                    return Some(Delivery {
                        job,
                        message: Some(message),
                        lease_failure: None,
                    });
                }
                Ok(None) => {
                    if !self.service_probe_pending {
                        self.recovered();
                    } else {
                        self.pause();
                    }
                }
                Err(_) => {
                    warn!(outcome = "receive_unavailable", "SQS receive paused");
                    self.pause();
                }
            }
        }
    }

    async fn wait_for_receive_resume(&mut self) -> Option<()> {
        loop {
            if self.control.stopping() {
                return None;
            }
            let Some(until) = self.pause_until else {
                return Some(());
            };
            tokio::select! {
                biased;
                () = self.control.cancelled() => return None,
                () = tokio::time::sleep_until(until) => {}
            }
            if let Source::Sqs(queue) = &self.source {
                let probe = tokio::select! {
                    biased;
                    () = self.control.cancelled() => return None,
                    result = bounded(queue.transport.probe(), API_TIMEOUT) => result,
                };
                if probe.is_err() {
                    self.pause();
                    continue;
                }
            }
            // Half-open: one job probes service recovery, not a drain of the backlog.
            self.pause_until = None;
            return Some(());
        }
    }

    fn recovered(&mut self) {
        self.circuit_failures = 0;
        self.pause_until = None;
        self.control.0.ready.store(true, Ordering::Release);
    }

    fn pause(&mut self) {
        self.circuit_failures = self.circuit_failures.saturating_add(1);
        let delay = retry_delay(self.circuit_failures, jitter_sample());
        self.pause_until = Some(Instant::now() + delay);
        self.control.0.ready.store(false, Ordering::Release);
        warn!(
            pause_seconds = delay.as_secs(),
            outcome = "dependency_circuit_open",
            "consumer paused; ingress publication remains independent"
        );
    }

    pub(crate) async fn process<F, Fut>(&mut self, delivery: Delivery, handler: F)
    where
        F: FnOnce(DomainJob) -> Fut,
        Fut: Future<Output = JobOutcome> + Send + 'static,
    {
        let Some(scope) = self.scope else {
            return;
        };
        let Delivery {
            job,
            message,
            lease_failure,
        } = delivery;
        let attempt = message.as_ref().map_or(1, |message| message.receive_count);
        let outcome = match (job, lease_failure) {
            (_, Some(reason)) => {
                warn!(
                    scope = scope.as_str(),
                    attempt,
                    outcome = reason,
                    "held receipt retained without handler execution"
                );
                JobOutcome::TransportUnavailable(reason)
            }
            (Ok(job), None) => {
                let key = job.idempotency_key.as_str().to_owned();
                let ordering_key = job.ordering_key.as_str().to_owned();
                let transport = match &self.source {
                    Source::Sqs(queue) => Some(queue.transport.as_ref()),
                    Source::Memory(_) => None,
                };
                let execution_started = Instant::now();
                let result = execute_tracked(
                    &self.control.0.cancelled_tasks,
                    transport,
                    message.as_ref(),
                    config::visibility(scope),
                    config::execution_budget(scope),
                    handler(job),
                )
                .await;
                info!(scope = scope.as_str(), idempotency_key = %key, %ordering_key, attempt,
                    execution_duration_ms = execution_started.elapsed().as_secs_f64() * 1000.0,
                    outcome = result.category(), "worker job attempt finished");
                result
            }
            (Err(_), None) => {
                warn!(
                    scope = scope.as_str(),
                    attempt,
                    outcome = "invalid_wire_job",
                    "poison retained for native SQS redrive"
                );
                JobOutcome::Invalid("invalid_wire_job")
            }
        };
        let settlement_failed = if let (Source::Sqs(queue), Some(message)) = (&self.source, message)
        {
            settle(queue.transport.as_ref(), &message, outcome)
                .await
                .is_err()
        } else {
            false
        };
        if settlement_failed {
            // Never rerun the handler to retry a delete. Visibility/redrive owns recovery.
            warn!(
                scope = scope.as_str(),
                attempt,
                outcome = "receipt_settlement_failed",
                "SQS receipt retained"
            );
        }
        // A lost receipt heartbeat does not prove a service outage, nor clear an earlier one.
        if matches!(outcome, JobOutcome::DependencyUnavailable(_)) {
            self.service_probe_pending = true;
        } else if matches!(outcome, JobOutcome::Complete(_)) {
            self.service_probe_pending = false;
        }
        if settlement_failed
            || matches!(
                outcome,
                JobOutcome::DependencyUnavailable(_) | JobOutcome::TransportUnavailable(_)
            )
            || (self.circuit_failures > 0 && !matches!(outcome, JobOutcome::Complete(_)))
        {
            // Poison, active claims and retries do not prove downstream recovery.
            self.pause();
        } else if matches!(outcome, JobOutcome::Complete(_)) {
            self.recovered();
        }
    }
}

async fn execute_tracked<F>(
    cancelled_tasks: &CancelledTasks,
    transport: Option<&dyn Transport>,
    message: Option<&Message>,
    visibility: Duration,
    budget: Duration,
    handler: F,
) -> JobOutcome
where
    F: Future<Output = JobOutcome> + Send + 'static,
{
    // Cancellation retains the join in the runtime supervisor, not just an abort request.
    let mut task = OwnedTasks::new(cancelled_tasks.clone());
    task.spawn(async move {
        tokio::time::timeout(budget, handler)
            .await
            .unwrap_or(JobOutcome::DependencyUnavailable("execution_timeout"))
    });
    let mut heartbeat = heartbeat_interval(visibility);
    loop {
        tokio::select! {
            biased;
            result = task.join_next() => return match result {
                Some(Ok(outcome)) => outcome,
                Some(Err(_)) | None => JobOutcome::Retry("handler_panicked_or_cancelled"),
            },
            _ = heartbeat.tick(), if transport.is_some() && message.is_some() => {
                if let (Some(transport), Some(message)) = (transport, message)
                    && bounded(transport.visibility(&message.receipt, visibility.as_secs() as i32), API_TIMEOUT).await.is_err()
                {
                    task.abort_all();
                    let mut outcome = JobOutcome::TransportUnavailable("heartbeat_failed");
                    while let Some(result) = task.join_next().await {
                        // A service failure may already have completed during the heartbeat call.
                        if let Ok(service_failure @ JobOutcome::DependencyUnavailable(_)) = result {
                            outcome = service_failure;
                        }
                    }
                    return outcome;
                }
            }
        }
    }
}

#[cfg(test)]
async fn execute_owned<F>(
    transport: Option<&dyn Transport>,
    message: Option<&Message>,
    visibility: Duration,
    budget: Duration,
    handler: F,
) -> JobOutcome
where
    F: Future<Output = JobOutcome> + Send + 'static,
{
    let cancelled = CancelledTasks::default();
    execute_tracked(&cancelled, transport, message, visibility, budget, handler).await
}

fn heartbeat_interval(visibility: Duration) -> tokio::time::Interval {
    let period = (visibility / 3).min(Duration::from_secs(30));
    let mut heartbeat = tokio::time::interval_at(Instant::now() + period, period);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat
}

async fn settle(
    transport: &dyn Transport,
    message: &Message,
    outcome: JobOutcome,
) -> Result<(), super::QueueError> {
    // execute_owned has returned: no heartbeat can race a retry visibility change or delete.
    if matches!(outcome, JobOutcome::Complete(_)) {
        for attempt in 0..3 {
            match bounded(transport.delete(&message.receipt), API_TIMEOUT).await {
                Ok(()) => return Ok(()),
                Err(error) if attempt == 2 => return Err(error),
                Err(_) => tokio::time::sleep(Duration::from_secs(1 << attempt)).await,
            }
        }
        return Err(super::QueueError::Unavailable);
    }
    let delay = match outcome {
        JobOutcome::RetryAfter(not_before) => {
            deferred_seconds(not_before, OffsetDateTime::now_utc())
        }
        JobOutcome::Retry(_)
        | JobOutcome::Invalid(_)
        | JobOutcome::DependencyUnavailable(_)
        | JobOutcome::TransportUnavailable(_) => {
            retry_delay(message.receive_count, jitter_sample()).as_secs() as i32
        }
        JobOutcome::Complete(_) => return Ok(()),
    };
    bounded(transport.visibility(&message.receipt, delay), API_TIMEOUT).await
}

fn deferred_seconds(not_before: OffsetDateTime, now: OffsetDateTime) -> i32 {
    let nanos = (not_before - now).whole_nanoseconds();
    ((nanos.max(0) + 999_999_999) / 1_000_000_000).clamp(1, 43_200) as i32
}
fn jitter_sample() -> u64 {
    std::collections::hash_map::RandomState::new().hash_one(std::time::SystemTime::now())
}
fn retry_delay(attempt: u32, sample: u64) -> Duration {
    let base = 30_u64
        .saturating_mul(1_u64 << attempt.saturating_sub(1).min(5))
        .min(900);
    let spread = (base / 2).min(900 - base);
    Duration::from_secs(base + sample % (spread + 1))
}

use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_listing_service::ports::ProductListingRawStreamId;
use product_service::{
    ports::PendingProductListingRawStreamCursor,
    use_cases::{
        NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionMode,
        NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionUseCase,
    },
};
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::{sync::watch, time::MissedTickBehavior};
use tracing::{info, warn};

const MAX_REVISIONS_PER_STREAM: u32 = 32;
const PENDING_STREAM_LIMIT: u32 = 100;
const MAX_PENDING_STREAM_CONTINUATIONS: usize = PENDING_STREAM_LIMIT as usize * 2;
const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(30);

fn reconciliation_interval() -> tokio::time::Interval {
    let mut reconciliation = tokio::time::interval(RECONCILIATION_INTERVAL);
    reconciliation.set_missed_tick_behavior(MissedTickBehavior::Skip);
    reconciliation
}

enum RawNormalizationPriority {
    Reconciliation,
    Cdc,
}

#[derive(Debug, Clone, Copy)]
enum ReconciliationTurn {
    Page,
    Continuation {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        reserve_global_page_slot: bool,
    },
}

#[derive(Debug, Default)]
struct ContinuationScheduling {
    unscheduled_continuation_count: usize,
    suppressed_continuation_count: usize,
}

#[derive(Default)]
struct ReconciliationState {
    pending_stream_cursor: Option<PendingProductListingRawStreamCursor>,
    continuation_streams: VecDeque<ProductListingRawStreamId>,
    continuation_turn_due: bool,
    global_page_slot_reserved: bool,
}

impl ReconciliationState {
    fn next_turn(&mut self) -> ReconciliationTurn {
        if self.global_page_slot_reserved {
            return ReconciliationTurn::Page;
        }
        if self.continuation_turn_due
            || self.continuation_streams.len() == MAX_PENDING_STREAM_CONTINUATIONS
        {
            self.continuation_turn_due = false;
            let reserve_global_page_slot =
                self.continuation_streams.len() == MAX_PENDING_STREAM_CONTINUATIONS;
            if let Some(product_listing_raw_stream_id) = self.continuation_streams.pop_front() {
                self.global_page_slot_reserved = reserve_global_page_slot;
                return ReconciliationTurn::Continuation {
                    product_listing_raw_stream_id,
                    reserve_global_page_slot,
                };
            }
        }
        ReconciliationTurn::Page
    }

    fn record_completed_turn(
        &mut self,
        turn: ReconciliationTurn,
        result: &NormalizeProductListingRawRevisionResult,
    ) -> ContinuationScheduling {
        let continuation_scheduling =
            self.schedule_for_turn(turn, result.continuation_stream_ids.iter().copied());
        if matches!(turn, ReconciliationTurn::Page)
            && continuation_scheduling.unscheduled_continuation_count == 0
        {
            self.pending_stream_cursor = result.next_pending_stream_cursor;
            self.global_page_slot_reserved = false;
        }
        self.schedule_next_turn(turn);
        continuation_scheduling
    }

    fn record_failed_turn(&mut self, turn: ReconciliationTurn) -> ContinuationScheduling {
        let continuation_scheduling = match turn {
            ReconciliationTurn::Page => ContinuationScheduling::default(),
            ReconciliationTurn::Continuation {
                product_listing_raw_stream_id,
                ..
            } => self.schedule_for_turn(turn, [product_listing_raw_stream_id]),
        };
        self.schedule_next_turn(turn);
        continuation_scheduling
    }

    fn schedule_for_turn(
        &mut self,
        turn: ReconciliationTurn,
        product_listing_raw_stream_ids: impl IntoIterator<Item = ProductListingRawStreamId>,
    ) -> ContinuationScheduling {
        let suppressed_stream_id = match turn {
            ReconciliationTurn::Continuation {
                product_listing_raw_stream_id,
                reserve_global_page_slot: true,
            } => Some(product_listing_raw_stream_id),
            ReconciliationTurn::Page
            | ReconciliationTurn::Continuation {
                reserve_global_page_slot: false,
                ..
            } => None,
        };
        let mut continuation_scheduling = ContinuationScheduling::default();
        for product_listing_raw_stream_id in product_listing_raw_stream_ids {
            if Some(product_listing_raw_stream_id) == suppressed_stream_id {
                continuation_scheduling.suppressed_continuation_count += 1;
                continue;
            }
            continuation_scheduling.unscheduled_continuation_count +=
                self.schedule([product_listing_raw_stream_id]);
        }
        continuation_scheduling
    }

    fn schedule_next_turn(&mut self, turn: ReconciliationTurn) {
        self.continuation_turn_due = matches!(turn, ReconciliationTurn::Page)
            && !self.global_page_slot_reserved
            && !self.continuation_streams.is_empty();
    }

    fn global_page_command(&self) -> NormalizeProductListingRawRevisionCommand {
        reconcile_command(
            self.pending_stream_cursor,
            self.global_pending_stream_limit(),
        )
    }

    fn global_pending_stream_limit(&self) -> u32 {
        let available_continuation_capacity =
            MAX_PENDING_STREAM_CONTINUATIONS - self.continuation_streams.len();
        PENDING_STREAM_LIMIT.min(available_continuation_capacity as u32)
    }

    fn schedule(
        &mut self,
        product_listing_raw_stream_ids: impl IntoIterator<Item = ProductListingRawStreamId>,
    ) -> usize {
        let mut unscheduled_continuations = 0;
        for product_listing_raw_stream_id in product_listing_raw_stream_ids {
            if self
                .continuation_streams
                .contains(&product_listing_raw_stream_id)
            {
                continue;
            }
            if self.continuation_streams.len() >= MAX_PENDING_STREAM_CONTINUATIONS {
                unscheduled_continuations += 1;
                continue;
            }
            self.continuation_streams
                .push_back(product_listing_raw_stream_id);
        }
        unscheduled_continuations
    }

    fn pending_stream_cursor_present(&self) -> bool {
        self.pending_stream_cursor.is_some()
    }

    fn continuation_stream_count(&self) -> usize {
        self.continuation_streams.len()
    }
}

pub async fn consume_product_listing_raw_normalization_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut receiver = receiver.into();
    let Some(_guard) = receiver.start(WorkerScope::ProductListingRawNormalization) else {
        return;
    };
    let control = receiver.control();
    let mut polling = receiver.into_polling();
    let mut reconciliation = reconciliation_interval();
    let mut reconciliation_state = ReconciliationState::default();
    let mut priority = RawNormalizationPriority::Reconciliation;

    loop {
        if *shutdown.borrow() || control.stopping() {
            log_shutdown(
                reconciliation_state.pending_stream_cursor_present(),
                reconciliation_state.continuation_stream_count(),
            );
            break;
        }

        match priority {
            RawNormalizationPriority::Reconciliation => {
                tokio::select! {
                    biased;
                    () = control.cancelled() => break,
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            log_shutdown(
                                reconciliation_state.pending_stream_cursor_present(),
                                reconciliation_state.continuation_stream_count(),
                            );
                            break;
                        }
                    }
                    _ = reconciliation.tick() => {
                        reconcile_pending_stream_turn(Arc::clone(&use_case), &mut reconciliation_state, &control).await;
                        priority = RawNormalizationPriority::Cdc;
                    }
                    () = polling.ready() => {
                        let Some((mut receiver, Some(job))) = polling.take().await else {
                            break;
                        };
                        if control.stopping() || *shutdown.borrow() {
                            break;
                        }
                        let use_case = Arc::clone(&use_case);
                        receiver.process(job, move |job| normalize_job(use_case, job)).await;
                        polling = receiver.into_polling();
                        priority = RawNormalizationPriority::Reconciliation;
                    }
                }
            }
            RawNormalizationPriority::Cdc => {
                tokio::select! {
                    biased;
                    () = control.cancelled() => break,
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            log_shutdown(
                                reconciliation_state.pending_stream_cursor_present(),
                                reconciliation_state.continuation_stream_count(),
                            );
                            break;
                        }
                    }
                    () = polling.ready() => {
                        let Some((mut receiver, Some(job))) = polling.take().await else {
                            break;
                        };
                        if control.stopping() || *shutdown.borrow() {
                            break;
                        }
                        let use_case = Arc::clone(&use_case);
                        receiver.process(job, move |job| normalize_job(use_case, job)).await;
                        polling = receiver.into_polling();
                        priority = RawNormalizationPriority::Reconciliation;
                    }
                    _ = reconciliation.tick() => {
                        reconcile_pending_stream_turn(Arc::clone(&use_case), &mut reconciliation_state, &control).await;
                        priority = RawNormalizationPriority::Cdc;
                    }
                }
            }
        }
    }
    polling.stop().await;
}

fn log_shutdown(pending_stream_cursor_present: bool, continuation_stream_count: usize) {
    info!(
        metric = "product_listing_raw_normalization_reconciliation",
        job_type = "product_listing_raw_normalization_reconciliation",
        reconciliation_runs = 0_u64,
        processed_revisions = 0_u64,
        normalization_failures = 0_u64,
        pending_stream_page_count = 0_u64,
        reconciliation_page = "not_run",
        pending_stream_cursor_present,
        reconciliation_continuation_stream_count = continuation_stream_count,
        outcome = "shutdown",
        "raw normalization consumer shutdown requested"
    );
}

async fn normalize_job(
    use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let Ok(command) = command_from_job(job) else {
        return JobOutcome::Invalid("unexpected_payload");
    };
    match use_case.execute(command).await {
        Ok(result) => {
            info!(
                processed_revisions = result.revisions.len(),
                normalization_failures = result.stream_failures.len(),
                "raw stream drain finished"
            );
            if !result.stream_failures.is_empty() {
                return JobOutcome::DependencyUnavailable("normalization_stream_failed");
            }
            if !result.continuation_stream_ids.is_empty() {
                return JobOutcome::Retry("normalization_continuation");
            }
            JobOutcome::Complete("stream_drained")
        }
        Err(error) => {
            use product_service::use_cases::NormalizeProductListingRawRevisionError as E;
            match error {
                E::InvalidLimit
                | E::InvalidPersistedState { .. }
                | E::UnsupportedStoredSchemaVersion
                | E::NormalizationConfigurationFailed { .. } => {
                    JobOutcome::Invalid("normalization_state_invalid")
                }
                _ => JobOutcome::DependencyUnavailable("normalization_unavailable"),
            }
        }
    }
}

async fn reconcile_pending_stream_turn(
    use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>,
    reconciliation_state: &mut ReconciliationState,
    control: &crate::queue::RuntimeControl,
) {
    let turn = reconciliation_state.next_turn();
    let (command, reconciliation_page) = match turn {
        ReconciliationTurn::Page => (
            reconciliation_state.global_page_command(),
            if reconciliation_state.pending_stream_cursor_present() {
                "global_cursor"
            } else {
                "global_initial"
            },
        ),
        ReconciliationTurn::Continuation {
            product_listing_raw_stream_id,
            ..
        } => (
            continuation_command(product_listing_raw_stream_id),
            "continuation",
        ),
    };
    let mut task = control.owned_tasks();
    task.spawn(async move { use_case.execute(command).await });
    let result = match tokio::time::timeout(Duration::from_secs(240), task.join_next()).await {
        Ok(Some(Ok(Ok(result)))) => Ok(result),
        _ => {
            task.abort_all();
            while task.join_next().await.is_some() {}
            Err(())
        }
    };

    match result {
        Ok(result) => {
            let continuation_scheduling = reconciliation_state.record_completed_turn(turn, &result);
            let processed_revisions = result.revisions.len() as u64;
            let normalization_failures = result.stream_failures.len() as u64;
            let pending_stream_page_count = result
                .pending_stream_page_count
                .map_or(0_u64, |count| count as u64);
            info!(
                metric = "product_listing_raw_normalization_reconciliation",
                job_type = "product_listing_raw_normalization_reconciliation",
                reconciliation_runs = 1_u64,
                processed_revisions,
                normalization_failures,
                pending_stream_page_count,
                oldest_pending_age_seconds = result.oldest_pending_age_seconds,
                reconciliation_page,
                pending_stream_cursor_present =
                    reconciliation_state.pending_stream_cursor_present(),
                reconciliation_continuation_stream_count =
                    reconciliation_state.continuation_stream_count(),
                unscheduled_continuation_count =
                    continuation_scheduling.unscheduled_continuation_count,
                suppressed_continuation_count =
                    continuation_scheduling.suppressed_continuation_count,
                outcome = "completed",
                "raw normalization reconciliation turn completed"
            );
            if continuation_scheduling.unscheduled_continuation_count > 0 {
                warn!(
                    metric = "product_listing_raw_normalization_reconciliation",
                    job_type = "product_listing_raw_normalization_reconciliation",
                    reconciliation_page,
                    pending_stream_cursor_present =
                        reconciliation_state.pending_stream_cursor_present(),
                    reconciliation_continuation_stream_count =
                        reconciliation_state.continuation_stream_count(),
                    unscheduled_continuation_count =
                        continuation_scheduling.unscheduled_continuation_count,
                    suppressed_continuation_count =
                        continuation_scheduling.suppressed_continuation_count,
                    outcome = "continuation_deferred",
                    "raw normalization continuation FIFO is full; global cursor retained"
                );
            }
        }
        Err(_) => {
            let continuation_scheduling = reconciliation_state.record_failed_turn(turn);
            warn!(
                metric = "product_listing_raw_normalization_reconciliation",
                job_type = "product_listing_raw_normalization_reconciliation",
                reconciliation_runs = 1_u64,
                processed_revisions = 0_u64,
                normalization_failures = 1_u64,
                pending_stream_page_count = 0_u64,
                reconciliation_page,
                pending_stream_cursor_present =
                    reconciliation_state.pending_stream_cursor_present(),
                reconciliation_continuation_stream_count =
                    reconciliation_state.continuation_stream_count(),
                unscheduled_continuation_count =
                    continuation_scheduling.unscheduled_continuation_count,
                suppressed_continuation_count =
                    continuation_scheduling.suppressed_continuation_count,
                error_code = "RECONCILIATION_FAILED",
                outcome = "retry_at_next_turn",
                "raw normalization reconciliation turn failed; a later interval will retry"
            );
        }
    }
}

fn command_from_job(
    job: DomainJob,
) -> Result<NormalizeProductListingRawRevisionCommand, ProductListingRawNormalizationWorkerError> {
    let DomainJobPayload::ProductListingRawRevision(revision) = job.payload else {
        return Err(ProductListingRawNormalizationWorkerError::UnexpectedJobPayload);
    };
    Ok(NormalizeProductListingRawRevisionCommand {
        mode: NormalizeProductListingRawRevisionMode::RawRevision {
            product_listing_raw_stream_id: revision.product_listing_raw_stream_id,
            product_listing_raw_revision_id: revision.product_listing_raw_revision_id,
            revision: revision.revision,
        },
        max_revisions_per_stream: MAX_REVISIONS_PER_STREAM,
        pending_stream_limit: PENDING_STREAM_LIMIT,
    })
}

fn reconcile_command(
    pending_stream_cursor: Option<PendingProductListingRawStreamCursor>,
    pending_stream_limit: u32,
) -> NormalizeProductListingRawRevisionCommand {
    let mode = match pending_stream_cursor {
        Some(pending_stream_cursor) => {
            NormalizeProductListingRawRevisionMode::ReconcileFromCursor {
                pending_stream_cursor,
            }
        }
        None => NormalizeProductListingRawRevisionMode::Reconcile,
    };
    NormalizeProductListingRawRevisionCommand {
        mode,
        max_revisions_per_stream: MAX_REVISIONS_PER_STREAM,
        pending_stream_limit,
    }
}

fn continuation_command(
    product_listing_raw_stream_id: ProductListingRawStreamId,
) -> NormalizeProductListingRawRevisionCommand {
    NormalizeProductListingRawRevisionCommand {
        mode: NormalizeProductListingRawRevisionMode::ReconcileContinuation {
            product_listing_raw_stream_id,
        },
        max_revisions_per_stream: MAX_REVISIONS_PER_STREAM,
        pending_stream_limit: PENDING_STREAM_LIMIT,
    }
}

#[derive(Debug, thiserror::Error)]
enum ProductListingRawNormalizationWorkerError {
    #[error("product listing raw normalization queue received an unexpected job payload")]
    UnexpectedJobPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        InMemoryQueueSender, QueueConfig, QueueConfigError,
        cdc::{IdempotencyKey, OrderingKey, ProductListingRawRevisionJob, WorkerQueue},
        in_memory_queue,
    };
    use product_listing_service::ports::{ProductListingRawRevisionId, ProductListingRawStreamId};
    use product_service::{
        ports::{PendingProductListingRawStreamCursor, ProductListingRawNormalizationOutcome},
        use_cases::{
            NormalizeProductListingRawRevisionError, NormalizeProductListingRawRevisionResult,
            NormalizedRawRevisionResult, ProductListingRawNormalizationStreamFailure,
        },
    };
    use std::{collections::VecDeque, sync::Arc, time::Duration};
    use tokio::{
        sync::{Mutex, mpsc, oneshot, watch},
        task::JoinHandle,
    };

    const UUID_V7_BASE: u128 = 0x0190_0000_0000_7000_8000_0000_0000_0000;

    fn uuid_v7(low_bits: u64) -> uuid::Uuid {
        uuid::Uuid::from_u128(UUID_V7_BASE | u128::from(low_bits))
    }

    fn raw_stream_id(low_bits: u64) -> ProductListingRawStreamId {
        ProductListingRawStreamId::try_from(uuid_v7(low_bits))
            .unwrap_or_else(|error| panic!("valid raw stream UUIDv7 fixture: {error}"))
    }

    fn raw_revision_id(low_bits: u64) -> ProductListingRawRevisionId {
        ProductListingRawRevisionId::try_from(uuid_v7(low_bits))
            .unwrap_or_else(|error| panic!("valid raw revision UUIDv7 fixture: {error}"))
    }

    #[test]
    fn should_map_raw_revision_job_to_normalization_command() {
        let product_listing_raw_stream_id = raw_stream_id(1);
        let product_listing_raw_revision_id = raw_revision_id(2);

        let command = command_from_job(DomainJob {
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
                revision: 2,
            }),
        });

        assert!(matches!(
            command,
            Ok(NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::RawRevision {
                    product_listing_raw_stream_id: actual_stream_id,
                    product_listing_raw_revision_id: actual_revision_id,
                    revision: 2,
                },
                max_revisions_per_stream: MAX_REVISIONS_PER_STREAM,
                pending_stream_limit: PENDING_STREAM_LIMIT,
            }) if actual_stream_id == product_listing_raw_stream_id
                && actual_revision_id == product_listing_raw_revision_id
        ));
    }

    #[test]
    fn should_build_bounded_reconciliation_command() {
        let command = reconcile_command(None, PENDING_STREAM_LIMIT);

        assert!(matches!(
            command,
            NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::Reconcile,
                max_revisions_per_stream: MAX_REVISIONS_PER_STREAM,
                pending_stream_limit: PENDING_STREAM_LIMIT,
            }
        ));
    }

    #[test]
    fn should_retain_global_cursor_when_continuation_fifo_is_full() {
        let current_cursor = pending_stream_cursor(1);
        let next_cursor = pending_stream_cursor(2);
        let mut state = ReconciliationState {
            pending_stream_cursor: Some(current_cursor),
            ..Default::default()
        };
        let queued_streams = (3..(MAX_PENDING_STREAM_CONTINUATIONS + 3))
            .map(|value| raw_stream_id(value as u64))
            .collect::<Vec<_>>();
        assert_eq!(0, state.schedule(queued_streams));

        let deferred_stream = raw_stream_id(999);
        let continuation_scheduling = state.record_completed_turn(
            ReconciliationTurn::Page,
            &NormalizeProductListingRawRevisionResult {
                next_pending_stream_cursor: Some(next_cursor),
                continuation_stream_ids: vec![deferred_stream],
                ..Default::default()
            },
        );

        assert_eq!(1, continuation_scheduling.unscheduled_continuation_count);
        assert_eq!(0, continuation_scheduling.suppressed_continuation_count);
        assert_eq!(Some(current_cursor), state.pending_stream_cursor);
        assert_eq!(
            MAX_PENDING_STREAM_CONTINUATIONS,
            state.continuation_stream_count()
        );
        assert!(matches!(
            state.next_turn(),
            ReconciliationTurn::Continuation { .. }
        ));
    }

    #[test]
    fn should_reach_later_global_page_when_saturated_fifo_reserves_capacity() {
        let current_cursor = pending_stream_cursor(1);
        let next_cursor = pending_stream_cursor(2);
        let popped_stream = raw_stream_id(3);
        let deferred_stream = raw_stream_id(999);
        let mut state = ReconciliationState {
            pending_stream_cursor: Some(current_cursor),
            continuation_turn_due: true,
            ..Default::default()
        };
        let queued_streams = std::iter::once(popped_stream)
            .chain(
                (4..(MAX_PENDING_STREAM_CONTINUATIONS + 3))
                    .map(|value| raw_stream_id(value as u64)),
            )
            .collect::<Vec<_>>();
        assert_eq!(0, state.schedule(queued_streams));

        let saturated_continuation = state.next_turn();
        assert!(matches!(
            saturated_continuation,
            ReconciliationTurn::Continuation {
                product_listing_raw_stream_id,
                reserve_global_page_slot: true,
            } if product_listing_raw_stream_id == popped_stream
        ));
        let continuation_scheduling = state.record_completed_turn(
            saturated_continuation,
            &NormalizeProductListingRawRevisionResult {
                continuation_stream_ids: vec![popped_stream],
                ..Default::default()
            },
        );

        assert_eq!(0, continuation_scheduling.unscheduled_continuation_count);
        assert_eq!(1, continuation_scheduling.suppressed_continuation_count);
        assert_eq!(Some(current_cursor), state.pending_stream_cursor);
        assert_eq!(
            MAX_PENDING_STREAM_CONTINUATIONS - 1,
            state.continuation_stream_count()
        );
        assert!(!state.continuation_streams.contains(&popped_stream));
        assert!(state.global_page_slot_reserved);

        let first_global_page = state.next_turn();
        assert!(matches!(first_global_page, ReconciliationTurn::Page));
        let first_global_command = state.global_page_command();
        assert_eq!(1, first_global_command.pending_stream_limit);
        assert_reconcile_from_cursor(first_global_command, current_cursor);
        let global_page_scheduling = state.record_completed_turn(
            first_global_page,
            &NormalizeProductListingRawRevisionResult {
                next_pending_stream_cursor: Some(next_cursor),
                continuation_stream_ids: vec![deferred_stream],
                ..Default::default()
            },
        );

        assert_eq!(0, global_page_scheduling.unscheduled_continuation_count);
        assert_eq!(0, global_page_scheduling.suppressed_continuation_count);
        assert_eq!(Some(next_cursor), state.pending_stream_cursor);
        assert_eq!(
            MAX_PENDING_STREAM_CONTINUATIONS,
            state.continuation_stream_count()
        );
        assert!(state.continuation_streams.contains(&deferred_stream));
        assert!(!state.global_page_slot_reserved);

        let next_continuation = state.next_turn();
        assert!(matches!(
            next_continuation,
            ReconciliationTurn::Continuation {
                reserve_global_page_slot: true,
                ..
            }
        ));
        let _continuation_scheduling = state.record_completed_turn(
            next_continuation,
            &NormalizeProductListingRawRevisionResult::default(),
        );

        let later_global_page = state.next_turn();
        assert!(matches!(later_global_page, ReconciliationTurn::Page));
        let later_global_command = state.global_page_command();
        assert_eq!(1, later_global_command.pending_stream_limit);
        assert_reconcile_from_cursor(later_global_command, next_cursor);
    }

    #[tokio::test(start_paused = true)]
    async fn should_continue_from_cursor_after_a_failure_only_reconciliation_page()
    -> Result<(), Box<dyn std::error::Error>> {
        let cursor = pending_stream_cursor(999);
        let (_sender, shutdown, mut commands, consumer) = start_consumer(
            1,
            VecDeque::from([
                ScriptedResponse::Success(reconciliation_result(Some(cursor))),
                ScriptedResponse::Success(reconciliation_result(None)),
            ]),
        )?;

        let first = next_command(&mut commands).await?;
        assert!(matches!(
            first.mode,
            NormalizeProductListingRawRevisionMode::Reconcile
        ));

        tokio::task::yield_now().await;
        tokio::time::advance(RECONCILIATION_INTERVAL).await;

        assert_reconcile_from_cursor(next_command(&mut commands).await?, cursor);

        shutdown_consumer(shutdown, consumer).await?;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_process_queued_cdc_job_after_a_reconciliation_page_before_another_due_page()
    -> Result<(), Box<dyn std::error::Error>> {
        let cursor = pending_stream_cursor(1);
        let next_cursor = pending_stream_cursor(2);
        let (first_page_release, first_page_wait) = oneshot::channel();
        let (sender, shutdown, mut commands, consumer) = start_consumer(
            1,
            VecDeque::from([
                ScriptedResponse::Blocked {
                    release: first_page_wait,
                    result: reconciliation_result_with_processed_revision(Some(cursor)),
                },
                ScriptedResponse::Success(NormalizeProductListingRawRevisionResult::default()),
                ScriptedResponse::Success(reconciliation_result_with_processed_revision(Some(
                    next_cursor,
                ))),
            ]),
        )?;

        let first = next_command(&mut commands).await?;
        assert!(matches!(
            first.mode,
            NormalizeProductListingRawRevisionMode::Reconcile
        ));

        sender.enqueue(raw_revision_job(7)).await?;
        tokio::time::advance(RECONCILIATION_INTERVAL).await;
        assert!(first_page_release.send(()).is_ok());

        let cdc = next_command(&mut commands).await?;
        assert!(matches!(
            cdc.mode,
            NormalizeProductListingRawRevisionMode::RawRevision { revision: 7, .. }
        ));

        assert_reconcile_from_cursor(next_command(&mut commands).await?, cursor);

        shutdown_consumer(shutdown, consumer).await?;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_alternate_capped_continuation_and_global_page_without_direct_cdc_moving_state()
    -> Result<(), Box<dyn std::error::Error>> {
        let cursor = pending_stream_cursor(4);
        let next_cursor = pending_stream_cursor(5);
        let continuation_stream_id = raw_stream_id(6);
        let (first_page_release, first_page_wait) = oneshot::channel();
        let (sender, shutdown, mut commands, consumer) = start_consumer(
            1,
            VecDeque::from([
                ScriptedResponse::Blocked {
                    release: first_page_wait,
                    result: reconciliation_result_with_capped_continuation(
                        Some(cursor),
                        continuation_stream_id,
                    ),
                },
                ScriptedResponse::Success(NormalizeProductListingRawRevisionResult::default()),
                ScriptedResponse::Success(NormalizeProductListingRawRevisionResult::default()),
                ScriptedResponse::Success(reconciliation_result(Some(next_cursor))),
            ]),
        )?;

        let first = next_command(&mut commands).await?;
        assert!(matches!(
            first.mode,
            NormalizeProductListingRawRevisionMode::Reconcile
        ));

        sender.enqueue(raw_revision_job(7)).await?;
        assert!(first_page_release.send(()).is_ok());

        let direct_cdc = next_command(&mut commands).await?;
        assert!(matches!(
            direct_cdc.mode,
            NormalizeProductListingRawRevisionMode::RawRevision { revision: 7, .. }
        ));

        tokio::task::yield_now().await;
        tokio::time::advance(RECONCILIATION_INTERVAL).await;
        let continuation = next_command(&mut commands).await?;
        assert!(matches!(
            continuation.mode,
            NormalizeProductListingRawRevisionMode::ReconcileContinuation {
                product_listing_raw_stream_id,
            } if product_listing_raw_stream_id == continuation_stream_id
        ));

        tokio::task::yield_now().await;
        tokio::time::advance(RECONCILIATION_INTERVAL).await;
        assert_reconcile_from_cursor(next_command(&mut commands).await?, cursor);

        shutdown_consumer(shutdown, consumer).await?;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_run_due_reconciliation_page_despite_sustained_queued_cdc_jobs()
    -> Result<(), Box<dyn std::error::Error>> {
        let (first_cdc_release, first_cdc_wait) = oneshot::channel();
        let (sender, shutdown, mut commands, consumer) = start_consumer(
            3,
            VecDeque::from([
                ScriptedResponse::Success(reconciliation_result(None)),
                ScriptedResponse::Blocked {
                    release: first_cdc_wait,
                    result: NormalizeProductListingRawRevisionResult::default(),
                },
                ScriptedResponse::Success(reconciliation_result(None)),
            ]),
        )?;

        let initial = next_command(&mut commands).await?;
        assert!(matches!(
            initial.mode,
            NormalizeProductListingRawRevisionMode::Reconcile
        ));
        tokio::task::yield_now().await;

        sender.enqueue(raw_revision_job(10)).await?;
        sender.enqueue(raw_revision_job(11)).await?;
        sender.enqueue(raw_revision_job(12)).await?;

        let first_cdc = next_command(&mut commands).await?;
        assert!(matches!(
            first_cdc.mode,
            NormalizeProductListingRawRevisionMode::RawRevision { revision: 10, .. }
        ));

        tokio::time::advance(RECONCILIATION_INTERVAL).await;
        assert!(first_cdc_release.send(()).is_ok());

        let due_reconciliation = next_command(&mut commands).await?;
        assert!(matches!(
            due_reconciliation.mode,
            NormalizeProductListingRawRevisionMode::Reconcile
        ));

        shutdown_consumer(shutdown, consumer).await?;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_retain_cursor_after_failed_turns_without_in_process_retries()
    -> Result<(), Box<dyn std::error::Error>> {
        let cursor = pending_stream_cursor(3);
        let (_sender, shutdown, mut commands, consumer) = start_consumer(
            1,
            VecDeque::from([
                ScriptedResponse::Success(reconciliation_result(Some(cursor))),
                ScriptedResponse::Failure,
                ScriptedResponse::Failure,
                ScriptedResponse::Failure,
                ScriptedResponse::Success(reconciliation_result(None)),
            ]),
        )?;

        let first = next_command(&mut commands).await?;
        assert!(matches!(
            first.mode,
            NormalizeProductListingRawRevisionMode::Reconcile
        ));

        tokio::task::yield_now().await;
        tokio::time::advance(RECONCILIATION_INTERVAL).await;
        assert_reconcile_from_cursor(next_command(&mut commands).await?, cursor);

        tokio::task::yield_now().await;
        tokio::time::advance(RECONCILIATION_INTERVAL).await;
        assert_reconcile_from_cursor(next_command(&mut commands).await?, cursor);

        tokio::task::yield_now().await;
        tokio::time::advance(RECONCILIATION_INTERVAL).await;
        assert_reconcile_from_cursor(next_command(&mut commands).await?, cursor);

        tokio::task::yield_now().await;
        tokio::time::advance(RECONCILIATION_INTERVAL).await;
        assert_reconcile_from_cursor(next_command(&mut commands).await?, cursor);

        shutdown_consumer(shutdown, consumer).await?;
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_skip_missed_reconciliation_ticks_instead_of_bursting() {
        let mut reconciliation = reconciliation_interval();

        reconciliation.tick().await;
        tokio::time::advance(RECONCILIATION_INTERVAL * 3).await;
        reconciliation.tick().await;

        assert!(
            tokio::time::timeout(Duration::from_millis(1), reconciliation.tick())
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn should_exit_on_shutdown_without_draining_queued_cdc_jobs()
    -> Result<(), Box<dyn std::error::Error>> {
        let (first_page_release, first_page_wait) = oneshot::channel();
        let (sender, shutdown, mut commands, consumer) = start_consumer(
            2,
            VecDeque::from([ScriptedResponse::Blocked {
                release: first_page_wait,
                result: reconciliation_result(None),
            }]),
        )?;

        let first = next_command(&mut commands).await?;
        assert!(matches!(
            first.mode,
            NormalizeProductListingRawRevisionMode::Reconcile
        ));

        sender.enqueue(raw_revision_job(8)).await?;
        sender.enqueue(raw_revision_job(9)).await?;
        let _previous_shutdown = shutdown.send_replace(true);
        drop(sender);
        assert!(first_page_release.send(()).is_ok());

        consumer.await?;
        assert!(commands.try_recv().is_err());
        Ok(())
    }

    struct ScriptedRawNormalizationUseCase {
        commands: mpsc::UnboundedSender<NormalizeProductListingRawRevisionCommand>,
        responses: Mutex<VecDeque<ScriptedResponse>>,
    }

    impl ScriptedRawNormalizationUseCase {
        fn new(
            commands: mpsc::UnboundedSender<NormalizeProductListingRawRevisionCommand>,
            responses: VecDeque<ScriptedResponse>,
        ) -> Self {
            Self {
                commands,
                responses: Mutex::new(responses),
            }
        }
    }

    enum ScriptedResponse {
        Success(NormalizeProductListingRawRevisionResult),
        Failure,
        Blocked {
            release: oneshot::Receiver<()>,
            result: NormalizeProductListingRawRevisionResult,
        },
    }

    #[async_trait::async_trait]
    impl NormalizeProductListingRawRevisionUseCase for ScriptedRawNormalizationUseCase {
        async fn execute(
            &self,
            command: NormalizeProductListingRawRevisionCommand,
        ) -> Result<NormalizeProductListingRawRevisionResult, NormalizeProductListingRawRevisionError>
        {
            self.commands
                .send(command)
                .map_err(|_| NormalizeProductListingRawRevisionError::InvalidLimit)?;
            let response = self.responses.lock().await.pop_front();
            match response {
                Some(ScriptedResponse::Success(result)) => Ok(result),
                Some(ScriptedResponse::Failure) => {
                    Err(NormalizeProductListingRawRevisionError::InvalidLimit)
                }
                Some(ScriptedResponse::Blocked { release, result }) => {
                    release
                        .await
                        .map_err(|_| NormalizeProductListingRawRevisionError::InvalidLimit)?;
                    Ok(result)
                }
                None => Ok(NormalizeProductListingRawRevisionResult::default()),
            }
        }
    }

    type ConsumerParts = (
        InMemoryQueueSender<DomainJob>,
        watch::Sender<bool>,
        mpsc::UnboundedReceiver<NormalizeProductListingRawRevisionCommand>,
        JoinHandle<()>,
    );

    fn start_consumer(
        queue_capacity: usize,
        responses: VecDeque<ScriptedResponse>,
    ) -> Result<ConsumerParts, QueueConfigError> {
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase> =
            Arc::new(ScriptedRawNormalizationUseCase::new(commands_tx, responses));
        let (sender, receiver) = in_memory_queue(QueueConfig::new(queue_capacity))?;
        let (shutdown, shutdown_rx) = watch::channel(false);
        let consumer = tokio::spawn(consume_product_listing_raw_normalization_queue(
            receiver,
            use_case,
            shutdown_rx,
        ));
        Ok((sender, shutdown, commands_rx, consumer))
    }

    async fn next_command(
        commands: &mut mpsc::UnboundedReceiver<NormalizeProductListingRawRevisionCommand>,
    ) -> Result<NormalizeProductListingRawRevisionCommand, Box<dyn std::error::Error>> {
        match commands.recv().await {
            Some(command) => Ok(command),
            None => Err("consumer stopped before executing normalization work".into()),
        }
    }

    async fn shutdown_consumer(
        shutdown: watch::Sender<bool>,
        consumer: JoinHandle<()>,
    ) -> Result<(), tokio::task::JoinError> {
        let _previous_shutdown = shutdown.send_replace(true);
        consumer.await
    }

    fn assert_reconcile_from_cursor(
        command: NormalizeProductListingRawRevisionCommand,
        cursor: PendingProductListingRawStreamCursor,
    ) {
        assert!(matches!(
            command.mode,
            NormalizeProductListingRawRevisionMode::ReconcileFromCursor {
                pending_stream_cursor
            } if pending_stream_cursor == cursor
        ));
    }

    fn pending_stream_cursor(low_bits: u64) -> PendingProductListingRawStreamCursor {
        PendingProductListingRawStreamCursor {
            oldest_pending_at: time::OffsetDateTime::UNIX_EPOCH,
            product_listing_raw_stream_id: raw_stream_id(low_bits),
        }
    }

    fn raw_revision_job(revision: u64) -> DomainJob {
        let product_listing_raw_stream_id = raw_stream_id(1);
        let product_listing_raw_revision_id = raw_revision_id(revision + 10);
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
                revision,
            }),
        }
    }

    fn reconciliation_result(
        next_pending_stream_cursor: Option<PendingProductListingRawStreamCursor>,
    ) -> NormalizeProductListingRawRevisionResult {
        NormalizeProductListingRawRevisionResult {
            stream_failures: vec![ProductListingRawNormalizationStreamFailure {
                product_listing_raw_stream_id: raw_stream_id(1),
                error_code: "TEST_FAILURE",
            }],
            pending_stream_page_count: Some(1),
            next_pending_stream_cursor,
            ..Default::default()
        }
    }

    fn reconciliation_result_with_processed_revision(
        next_pending_stream_cursor: Option<PendingProductListingRawStreamCursor>,
    ) -> NormalizeProductListingRawRevisionResult {
        NormalizeProductListingRawRevisionResult {
            revisions: vec![NormalizedRawRevisionResult {
                product_listing_raw_stream_id: raw_stream_id(1),
                revision: 1,
                outcome: ProductListingRawNormalizationOutcome::Applied,
            }],
            pending_stream_page_count: Some(1),
            next_pending_stream_cursor,
            ..Default::default()
        }
    }

    fn reconciliation_result_with_capped_continuation(
        next_pending_stream_cursor: Option<PendingProductListingRawStreamCursor>,
        product_listing_raw_stream_id: ProductListingRawStreamId,
    ) -> NormalizeProductListingRawRevisionResult {
        NormalizeProductListingRawRevisionResult {
            pending_stream_page_count: Some(1),
            next_pending_stream_cursor,
            continuation_stream_ids: vec![product_listing_raw_stream_id],
            ..Default::default()
        }
    }
}

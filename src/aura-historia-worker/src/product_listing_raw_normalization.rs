use crate::{
    InMemoryQueueReceiver,
    cdc::{DomainJob, DomainJobPayload},
    retry::{InMemoryDeadLetterQueue, RetryConfig, run_with_retry},
};
use application::error::{BoxError, box_error};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionMode,
    NormalizeProductListingRawRevisionUseCase,
};
use std::{sync::Arc, time::Duration};
use tracing::{error, info, warn};

const MAX_REVISIONS_PER_STREAM: u32 = 32;
const PENDING_STREAM_LIMIT: u32 = 100;
const RECONCILIATION_INTERVAL: Duration = Duration::from_secs(30);

pub async fn consume_product_listing_raw_normalization_queue(
    mut receiver: InMemoryQueueReceiver<DomainJob>,
    use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>,
) {
    let job_dead_letters = InMemoryDeadLetterQueue::new();
    let reconciliation_dead_letters = InMemoryDeadLetterQueue::new();
    let mut reconciliation = tokio::time::interval(RECONCILIATION_INTERVAL);

    loop {
        tokio::select! {
            _ = reconciliation.tick() => {
                reconcile_pending_streams(Arc::clone(&use_case), &reconciliation_dead_letters).await;
            }
            job = receiver.recv() => {
                let Some(job) = job else {
                    return;
                };
                normalize_job(Arc::clone(&use_case), &job_dead_letters, job).await;
            }
        }
    }
}

async fn normalize_job(
    use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>,
    dead_letters: &InMemoryDeadLetterQueue<DomainJob>,
    job: DomainJob,
) {
    let idempotency_key = job.idempotency_key.as_str().to_owned();
    let ordering_key = job.ordering_key.as_str().to_owned();
    let result = run_with_retry(job, RetryConfig::default(), dead_letters, move |job| {
        let use_case = Arc::clone(&use_case);
        async move { execute_job(use_case, job).await }
    })
    .await;

    match result {
        Ok(result) => info!(
            metric = "product_listing_raw_normalization_job",
            job_type = "product_listing_raw_normalization",
            %idempotency_key,
            %ordering_key,
            processed_revisions = result.revisions.len(),
            normalization_failures = 0_u64,
            "product listing raw normalization job completed"
        ),
        Err(_) => error!(
            metric = "product_listing_raw_normalization_job",
            job_type = "product_listing_raw_normalization",
            %idempotency_key,
            %ordering_key,
            normalization_failures = 1_u64,
            error_code = "RETRY_EXHAUSTED",
            outcome = "dead_lettered_in_memory",
            "product listing raw normalization job failed"
        ),
    }
}

async fn reconcile_pending_streams(
    use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>,
    dead_letters: &InMemoryDeadLetterQueue<()>,
) {
    loop {
        let use_case_for_retry = Arc::clone(&use_case);
        let result = run_with_retry((), RetryConfig::default(), dead_letters, move |_| {
            let use_case = Arc::clone(&use_case_for_retry);
            async move {
                use_case
                    .execute(reconcile_command())
                    .await
                    .map_err(box_error)
            }
        })
        .await;

        match result {
            Ok(result) if result.revisions.is_empty() => {
                info!(
                    metric = "product_listing_raw_normalization_reconciliation",
                    job_type = "product_listing_raw_normalization_reconciliation",
                    reconciliation_runs = 1_u64,
                    processed_revisions = 0_u64,
                    pending_stream_page_count = result.pending_stream_page_count,
                    oldest_pending_age_seconds = result.oldest_pending_age_seconds,
                    "raw normalization reconciliation found no pending work"
                );
                return;
            }
            Ok(result) => {
                let processed_revisions = result.revisions.len();
                info!(
                    metric = "product_listing_raw_normalization_reconciliation",
                    job_type = "product_listing_raw_normalization_reconciliation",
                    reconciliation_runs = 1_u64,
                    processed_revisions,
                    pending_stream_page_count = result.pending_stream_page_count,
                    oldest_pending_age_seconds = result.oldest_pending_age_seconds,
                    "raw normalization reconciliation processed pending work"
                );
                tokio::task::yield_now().await;
            }
            Err(_) => {
                warn!(
                    metric = "product_listing_raw_normalization_reconciliation",
                    job_type = "product_listing_raw_normalization_reconciliation",
                    reconciliation_runs = 1_u64,
                    normalization_failures = 1_u64,
                    error_code = "RETRY_EXHAUSTED",
                    outcome = "retry_exhausted",
                    "raw normalization reconciliation failed; a later interval will retry"
                );
                return;
            }
        }
    }
}

async fn execute_job(
    use_case: Arc<dyn NormalizeProductListingRawRevisionUseCase>,
    job: DomainJob,
) -> Result<product_service::use_cases::NormalizeProductListingRawRevisionResult, BoxError> {
    let command = command_from_job(job).map_err(box_error)?;
    use_case.execute(command).await.map_err(box_error)
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

fn reconcile_command() -> NormalizeProductListingRawRevisionCommand {
    NormalizeProductListingRawRevisionCommand {
        mode: NormalizeProductListingRawRevisionMode::Reconcile,
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
    use crate::cdc::{IdempotencyKey, OrderingKey, ProductListingRawRevisionJob, WorkerQueue};
    use product_listing_service::ports::{ProductListingRawRevisionId, ProductListingRawStreamId};

    #[test]
    fn should_map_raw_revision_job_to_normalization_command() {
        let product_listing_raw_stream_id = ProductListingRawStreamId::from_uuid(uuid::Uuid::nil());
        let product_listing_raw_revision_id =
            ProductListingRawRevisionId::from_uuid(uuid::Uuid::max());

        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::ProductListingRawNormalization,
            idempotency_key: IdempotencyKey::new("product-listing-raw-revision:test"),
            ordering_key: OrderingKey::new("product-listing-raw-stream:test"),
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
        let command = reconcile_command();

        assert!(matches!(
            command,
            NormalizeProductListingRawRevisionCommand {
                mode: NormalizeProductListingRawRevisionMode::Reconcile,
                max_revisions_per_stream: MAX_REVISIONS_PER_STREAM,
                pending_stream_limit: PENDING_STREAM_LIMIT,
            }
        ));
    }
}

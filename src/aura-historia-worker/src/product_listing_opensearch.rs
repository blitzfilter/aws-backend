use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use product_listing_service::use_cases::{
    ProjectProductListingCommand, ProjectProductListingError, ProjectProductListingOutcome,
    ProjectProductListingUseCase,
};
use std::sync::Arc;

pub async fn consume_product_listing_opensearch_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn ProjectProductListingUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::ProductListingOpenSearch, move |job| {
            execute_job(use_case.clone(), job)
        })
        .await;
}

async fn execute_job(
    use_case: Arc<dyn ProjectProductListingUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return JobOutcome::Invalid("unexpected_payload");
    };
    match use_case
        .execute(ProjectProductListingCommand {
            event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => projection_outcome(result.outcome),
        Err(ProjectProductListingError::SaleObservationFxSnapshotMissing) => {
            JobOutcome::Retry("sale_snapshot_missing")
        }
        Err(ProjectProductListingError::SaleObservationFxSnapshotInvalid { .. }) => {
            JobOutcome::Invalid("sale_snapshot_invalid")
        }
        Err(_) => JobOutcome::DependencyUnavailable("projection_unavailable"),
    }
}
fn projection_outcome(outcome: ProjectProductListingOutcome) -> JobOutcome {
    match outcome {
        ProjectProductListingOutcome::Applied => JobOutcome::Complete("applied"),
        ProjectProductListingOutcome::Deleted => JobOutcome::Complete("deleted"),
        ProjectProductListingOutcome::Stale => JobOutcome::Complete("stale"),
        // Absence of the committed event/source is not evidence that its projection was removed.
        ProjectProductListingOutcome::MissingSource => JobOutcome::Retry("missing_source"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_retain_missing_source_but_complete_guarded_projection_outcomes() {
        assert_eq!(
            JobOutcome::Retry("missing_source"),
            projection_outcome(ProjectProductListingOutcome::MissingSource)
        );
        for outcome in [
            ProjectProductListingOutcome::Applied,
            ProjectProductListingOutcome::Deleted,
            ProjectProductListingOutcome::Stale,
        ] {
            assert!(matches!(
                projection_outcome(outcome),
                JobOutcome::Complete(_)
            ));
        }
    }
}

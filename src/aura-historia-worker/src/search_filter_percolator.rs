use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use search_filter_service::use_cases::{
    MatchProductListingEventCommand, MatchProductListingEventError,
    MatchProductListingEventOutcome, MatchProductListingEventUseCase,
};
use std::sync::Arc;

pub async fn consume_search_filter_percolator_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn MatchProductListingEventUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::SearchFilterPercolator, move |job| {
            execute_job(use_case.clone(), job)
        })
        .await;
}

async fn execute_job(
    use_case: Arc<dyn MatchProductListingEventUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return JobOutcome::Invalid("unexpected_payload");
    };
    match use_case
        .execute(MatchProductListingEventCommand {
            origin_event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        })
        .await
    {
        Ok(result) => {
            tracing::info!(
                percolated_count = result.percolated_count,
                persisted_match_count = result.persisted_match_count,
                enhanced_evaluation_failure_count = result.enhanced_evaluation_failure_count,
                "percolation completed"
            );
            percolator_outcome(result.outcome)
        }
        Err(error) => {
            use MatchProductListingEventError as E;
            match error {
                E::ProductListingSourceStateInvalid { .. }
                | E::ProductListingSourceMismatch
                | E::SaleSnapshotStateInvalid { .. }
                | E::EventSnapshotStateInvalid { .. }
                | E::EventValuationConversionFailed { .. }
                | E::CandidateStateInvalid { .. }
                | E::PersistedMatchStateInvalid { .. } => {
                    JobOutcome::Invalid("percolator_state_invalid")
                }
                E::SaleSnapshotNotFound { .. } | E::EventSnapshotNotFound { .. } => {
                    JobOutcome::Retry("valuation_snapshot_missing")
                }
                _ => JobOutcome::DependencyUnavailable("percolator_unavailable"),
            }
        }
    }
}
fn percolator_outcome(outcome: MatchProductListingEventOutcome) -> JobOutcome {
    match outcome {
        MatchProductListingEventOutcome::Processed => JobOutcome::Complete("processed"),
        MatchProductListingEventOutcome::DuplicateAlreadyPersisted => {
            JobOutcome::Complete("duplicate")
        }
        MatchProductListingEventOutcome::StaleSourceSkipped => JobOutcome::Complete("stale"),
        MatchProductListingEventOutcome::InactiveSourceSkipped => JobOutcome::Complete("withdrawn"),
        MatchProductListingEventOutcome::IgnoredEventType => JobOutcome::Complete("ignored_event"),
        MatchProductListingEventOutcome::SourceNotFound => JobOutcome::Retry("missing_source"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_not_ack_missing_source_or_invalidate_historical_work_in_transport() {
        assert_eq!(
            JobOutcome::Retry("missing_source"),
            percolator_outcome(MatchProductListingEventOutcome::SourceNotFound)
        );
        for outcome in [
            MatchProductListingEventOutcome::Processed,
            MatchProductListingEventOutcome::DuplicateAlreadyPersisted,
            MatchProductListingEventOutcome::StaleSourceSkipped,
            MatchProductListingEventOutcome::InactiveSourceSkipped,
            MatchProductListingEventOutcome::IgnoredEventType,
        ] {
            assert!(matches!(
                percolator_outcome(outcome),
                JobOutcome::Complete(_)
            ));
        }
    }
}

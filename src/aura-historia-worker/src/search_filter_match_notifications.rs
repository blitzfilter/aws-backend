use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationError,
    GenerateSearchFilterMatchNotificationResult, GenerateSearchFilterMatchNotificationUseCase,
};
use std::sync::Arc;

pub async fn consume_search_filter_match_notification_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::SearchFilterMatchNotification, move |job| {
            execute_job(use_case.clone(), job)
        })
        .await;
}
async fn execute_job(
    use_case: Arc<dyn GenerateSearchFilterMatchNotificationUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let Ok(command) = command_from_job(job) else {
        return JobOutcome::Invalid("match_metadata_invalid");
    };
    match use_case.execute(command).await {
        Ok(result) => notification_outcome(result),
        Err(error) => {
            use GenerateSearchFilterMatchNotificationError as E;
            match error {
                E::MatchSourceStateInvalid { .. }
                | E::ProductListingSourceStateInvalid { .. }
                | E::ProductListingSourceMismatch
                | E::ContentAssessmentStateInvalid { .. } => {
                    JobOutcome::Invalid("match_notification_state_invalid")
                }
                _ => JobOutcome::DependencyUnavailable("match_notification_unavailable"),
            }
        }
    }
}
fn notification_outcome(result: GenerateSearchFilterMatchNotificationResult) -> JobOutcome {
    use GenerateSearchFilterMatchNotificationResult as R;
    match result {
        R::Created => JobOutcome::Complete("inserted"),
        R::AlreadyExists => JobOutcome::Complete("duplicate"),
        R::SuppressedByQuota => JobOutcome::Complete("suppressed_by_quota"),
        // User deletion is a terminal recipient suppression, not missing historical business truth.
        R::SuppressedForMissingUser => JobOutcome::Complete("missing_user"),
        R::SuppressedForWithdrawnProductListing => JobOutcome::Complete("withdrawn"),
        R::SuppressedForStaleMatch => JobOutcome::Complete("stale_match"),
        R::SuppressedForMissingMatch => JobOutcome::Retry("missing_match"),
        R::SuppressedForMissingProductListing => JobOutcome::Retry("missing_product"),
    }
}
fn command_from_job(
    job: DomainJob,
) -> Result<GenerateSearchFilterMatchNotificationCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::SearchFilterMatchCreated(change) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(GenerateSearchFilterMatchNotificationCommand {
        user_id: change
            .user_id
            .as_str()
            .try_into()
            .map_err(|_| crate::jobs::InvalidJob)?,
        search_filter_id: change
            .user_search_filter_id
            .as_str()
            .try_into()
            .map_err(|_| crate::jobs::InvalidJob)?,
        product_listing_id: change
            .product_listing_id
            .as_str()
            .try_into()
            .map_err(|_| crate::jobs::InvalidJob)?,
        origin_event_id: change
            .origin_event_id
            .as_str()
            .try_into()
            .map_err(|_| crate::jobs::InvalidJob)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_retain_missing_historical_facts_and_complete_semantic_suppression() {
        use GenerateSearchFilterMatchNotificationResult as R;
        for result in [
            R::SuppressedForMissingMatch,
            R::SuppressedForMissingProductListing,
        ] {
            assert!(matches!(notification_outcome(result), JobOutcome::Retry(_)));
        }
        for result in [
            R::Created,
            R::AlreadyExists,
            R::SuppressedByQuota,
            R::SuppressedForMissingUser,
            R::SuppressedForWithdrawnProductListing,
            R::SuppressedForStaleMatch,
        ] {
            assert!(matches!(
                notification_outcome(result),
                JobOutcome::Complete(_)
            ));
        }
    }
}

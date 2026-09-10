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
        user_id: change.user_id,
        search_filter_id: change.user_search_filter_id,
        product_listing_id: change.product_listing_id,
        origin_event_id: change.origin_event_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, SearchFilterMatchCreatedJob, WorkerQueue};
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;
    use search_filter_core::user_search_filter_id::UserSearchFilterId;
    use user_core::user_id::UserId;
    use uuid::Uuid;

    #[test]
    fn should_map_typed_match_job_to_notification_command() -> Result<(), Box<dyn std::error::Error>>
    {
        let user_id = UserId::try_from(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001))?;
        let search_filter_id = UserSearchFilterId::try_from(Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0006,
        ))?;
        let product_listing_id =
            ProductListingId::try_from(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0003))?;
        let origin_event_id =
            EventId::try_from(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0004))?;

        assert_eq!("usr_01j0000000e008000000000001", user_id.to_string());
        assert_eq!(
            "sf_01j0000000e008000000000006",
            search_filter_id.to_string()
        );
        assert_eq!(
            "pl_01j0000000e008000000000003",
            product_listing_id.to_string()
        );
        assert_eq!(
            "evt_01j0000000e008000000000004",
            origin_event_id.to_string()
        );

        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::SearchFilterMatchNotification,
            idempotency_key: IdempotencyKey::new(format!(
                "search-filter-match:{user_id}:{search_filter_id}:{product_listing_id}:{origin_event_id}"
            )),
            ordering_key: OrderingKey::new(format!("user:{user_id}")),
            payload: DomainJobPayload::SearchFilterMatchCreated(SearchFilterMatchCreatedJob {
                user_id,
                user_search_filter_id: search_filter_id,
                product_listing_id,
                origin_event_id,
            }),
        });

        assert_eq!(
            Ok(GenerateSearchFilterMatchNotificationCommand {
                user_id,
                search_filter_id,
                product_listing_id,
                origin_event_id,
            }),
            command
        );
        Ok(())
    }

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

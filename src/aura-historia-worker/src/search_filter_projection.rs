use crate::{
    WorkerScope,
    cdc::{CdcOperation, DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeCommand, ProjectSearchFilterChangeError,
    ProjectSearchFilterChangeUseCase, SearchFilterProjectionOperation,
};
use std::sync::Arc;

pub async fn consume_search_filter_projection_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    handler: Arc<dyn ProjectSearchFilterChangeUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::SearchFilterProjection, move |job| {
            project_search_filter_change(handler.clone(), job)
        })
        .await;
}
async fn project_search_filter_change(
    handler: Arc<dyn ProjectSearchFilterChangeUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let Ok(command) = command_from_job(job) else {
        return JobOutcome::Invalid("projection_metadata_invalid");
    };
    match handler.execute(command).await {
        Ok(result) => {
            // Missing upsert source is handled by a target-side versioned tombstone in the use case.
            tracing::info!(outcome = ?result.outcome, "search filter projection write completed");
            JobOutcome::Complete("projection_written_or_stale")
        }
        Err(
            ProjectSearchFilterChangeError::InvalidSourceVersion
            | ProjectSearchFilterChangeError::DeleteVersionOverflow
            | ProjectSearchFilterChangeError::InvalidPersistedState { .. },
        ) => JobOutcome::Invalid("projection_state_invalid"),
        Err(
            ProjectSearchFilterChangeError::ReadFailed { .. }
            | ProjectSearchFilterChangeError::WriteFailed { .. },
        ) => JobOutcome::DependencyUnavailable("projection_unavailable"),
    }
}
fn command_from_job(
    job: DomainJob,
) -> Result<ProjectSearchFilterChangeCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::SearchFilterChanged(change) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    let search_filter_id = UserSearchFilterId::try_from(change.user_search_filter_id.as_str())
        .map_err(|_| crate::jobs::InvalidJob)?;
    if change.version <= 0 {
        return Err(crate::jobs::InvalidJob);
    }
    Ok(ProjectSearchFilterChangeCommand {
        search_filter_id,
        source_version: change.version,
        operation: match change.operation {
            CdcOperation::Insert | CdcOperation::Update => SearchFilterProjectionOperation::Upsert,
            CdcOperation::Delete => SearchFilterProjectionOperation::Delete,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, SearchFilterChangedJob, WorkerQueue};
    #[test]
    fn should_map_insert_update_and_delete_without_inventing_projection_completion() {
        let id = UserSearchFilterId::new();
        for (operation, expected) in [
            (
                CdcOperation::Insert,
                SearchFilterProjectionOperation::Upsert,
            ),
            (
                CdcOperation::Update,
                SearchFilterProjectionOperation::Upsert,
            ),
            (
                CdcOperation::Delete,
                SearchFilterProjectionOperation::Delete,
            ),
        ] {
            let command = command_from_job(DomainJob {
                target_queue: WorkerQueue::SearchFilterOpenSearch,
                idempotency_key: IdempotencyKey::new(format!("search-filter:{id}:3:{operation}")),
                ordering_key: OrderingKey::new(format!("search-filter:{id}")),
                payload: DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
                    user_id: "10000000-0000-0000-0000-000000000001".into(),
                    user_search_filter_id: id.to_string(),
                    version: 3,
                    operation,
                }),
            });
            assert_eq!(
                Ok(ProjectSearchFilterChangeCommand {
                    search_filter_id: id,
                    source_version: 3,
                    operation: expected
                }),
                command
            );
        }
    }
}

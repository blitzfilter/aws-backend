use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use product_listing_service::use_cases::{
    AssessProductListingContentCommand, AssessProductListingContentEventError,
    AssessProductListingContentEventOutcome, AssessProductListingContentEventUseCase,
};
use std::sync::Arc;

pub async fn consume_product_content_assessment_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn AssessProductListingContentEventUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::ProductListingContentAssessment, move |job| {
            execute_job(use_case.clone(), job)
        })
        .await;
}
async fn execute_job(
    use_case: Arc<dyn AssessProductListingContentEventUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let Ok(command) = command_from_job(job) else {
        return JobOutcome::Invalid("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-content-assessment:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => assessment_outcome(result.outcome),
        Err(AssessProductListingContentEventError::ServiceOrSystemPrincipalRequired) => {
            JobOutcome::Invalid("system_principal_required")
        }
        Err(_) => JobOutcome::DependencyUnavailable("assessment_unavailable"),
    }
}
fn assessment_outcome(outcome: AssessProductListingContentEventOutcome) -> JobOutcome {
    use AssessProductListingContentEventOutcome as O;
    match outcome {
        O::Applied => JobOutcome::Complete("applied"),
        O::Cleared => JobOutcome::Complete("cleared"),
        O::Duplicate => JobOutcome::Complete("duplicate"),
        O::Stale => JobOutcome::Complete("stale"),
        O::IgnoredEvent => JobOutcome::Complete("ignored_event"),
        O::ProductListingNotFound => JobOutcome::Retry("missing_source"),
    }
}
fn command_from_job(
    job: DomainJob,
) -> Result<AssessProductListingContentCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(AssessProductListingContentCommand {
        event_id: event.event_id,
        product_listing_id: event.product_listing_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::{IdempotencyKey, OrderingKey, ProductListingEventJob, WorkerQueue};
    use domain_primitives::event_id::EventId;
    use product_listing_core::product_listing_id::ProductListingId;
    #[test]
    fn should_map_product_event_job_to_content_assessment_command() {
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();
        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::ProductListingContentAssessment,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        });
        assert!(
            matches!(command, Ok(AssessProductListingContentCommand { event_id: actual, product_listing_id: product }) if actual == event_id && product == product_listing_id)
        );
    }
    #[test]
    fn should_retain_missing_assessment_source() {
        assert_eq!(
            JobOutcome::Retry("missing_source"),
            assessment_outcome(AssessProductListingContentEventOutcome::ProductListingNotFound)
        );
    }
}

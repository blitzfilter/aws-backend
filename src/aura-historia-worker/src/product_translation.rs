use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use product_listing_service::use_cases::{
    TranslateProductListingCommand, TranslateProductListingEventError,
    TranslateProductListingEventOutcome, TranslateProductListingEventUseCase,
};
use std::sync::Arc;

pub async fn consume_product_translation_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn TranslateProductListingEventUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::ProductListingTranslation, move |job| {
            execute_job(use_case.clone(), job)
        })
        .await;
}
async fn execute_job(
    use_case: Arc<dyn TranslateProductListingEventUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let Ok(command) = command_from_job(job) else {
        return JobOutcome::Invalid("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-translation:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => translation_outcome(result.outcome),
        Err(TranslateProductListingEventError::ServiceOrSystemPrincipalRequired) => {
            JobOutcome::Invalid("system_principal_required")
        }
        Err(_) => JobOutcome::DependencyUnavailable("translation_unavailable"),
    }
}
fn translation_outcome(outcome: TranslateProductListingEventOutcome) -> JobOutcome {
    use TranslateProductListingEventOutcome as O;
    match outcome {
        O::Applied => JobOutcome::Complete("applied"),
        O::Duplicate => JobOutcome::Complete("duplicate"),
        O::Stale => JobOutcome::Complete("stale"),
        O::IgnoredEvent => JobOutcome::Complete("ignored_event"),
        O::MissingTitle => JobOutcome::Complete("missing_title"),
        O::MissingTitleLanguage => JobOutcome::Complete("missing_title_language"),
        O::EmptyTitle => JobOutcome::Complete("empty_title"),
        O::ProductListingNotFound => JobOutcome::Retry("missing_source"),
    }
}
fn command_from_job(
    job: DomainJob,
) -> Result<TranslateProductListingCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(TranslateProductListingCommand {
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
    fn should_map_product_event_job_to_translation_command() {
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();
        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::ProductListingTranslate,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        });
        assert!(
            matches!(command, Ok(TranslateProductListingCommand { event_id: actual, product_listing_id: product }) if actual == event_id && product == product_listing_id)
        );
    }
    #[test]
    fn should_retain_missing_translation_source() {
        assert_eq!(
            JobOutcome::Retry("missing_source"),
            translation_outcome(TranslateProductListingEventOutcome::ProductListingNotFound)
        );
    }
}

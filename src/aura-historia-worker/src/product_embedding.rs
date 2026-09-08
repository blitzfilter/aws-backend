use crate::{
    WorkerScope,
    cdc::{DomainJob, DomainJobPayload},
    queue::{JobOutcome, WorkerQueueReceiver},
};
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use product_listing_service::use_cases::{
    EmbedProductListingCommand, EmbedProductListingEventError, EmbedProductListingEventOutcome,
    EmbedProductListingEventUseCase,
};
use std::sync::Arc;

pub async fn consume_product_embedding_queue(
    receiver: impl Into<WorkerQueueReceiver>,
    use_case: Arc<dyn EmbedProductListingEventUseCase>,
) {
    receiver
        .into()
        .run(WorkerScope::ProductListingEmbedding, move |job| {
            execute_job(use_case.clone(), job)
        })
        .await;
}
async fn execute_job(
    use_case: Arc<dyn EmbedProductListingEventUseCase>,
    job: DomainJob,
) -> JobOutcome {
    let Ok(command) = command_from_job(job) else {
        return JobOutcome::Invalid("unexpected_payload");
    };
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new(format!("product-embedding:{}", command.event_id)),
        correlation_id: CorrelationId::new(command.event_id.to_string()),
    };
    match use_case.execute(&context, command).await {
        Ok(result) => embedding_outcome(result.outcome),
        Err(
            EmbedProductListingEventError::ServiceOrSystemPrincipalRequired
            | EmbedProductListingEventError::InvalidInput { .. },
        ) => JobOutcome::Invalid("embedding_input_invalid"),
        Err(_) => JobOutcome::DependencyUnavailable("embedding_unavailable"),
    }
}
fn embedding_outcome(outcome: EmbedProductListingEventOutcome) -> JobOutcome {
    use EmbedProductListingEventOutcome as O;
    match outcome {
        O::Applied => JobOutcome::Complete("applied"),
        O::Duplicate => JobOutcome::Complete("duplicate"),
        O::Stale => JobOutcome::Complete("stale"),
        O::IgnoredEvent => JobOutcome::Complete("ignored_event"),
        O::MissingTitle => JobOutcome::Complete("missing_title"),
        O::ProductListingNotFound => JobOutcome::Retry("missing_source"),
    }
}
fn command_from_job(job: DomainJob) -> Result<EmbedProductListingCommand, crate::jobs::InvalidJob> {
    let DomainJobPayload::ProductListingEvent(event) = job.payload else {
        return Err(crate::jobs::InvalidJob);
    };
    Ok(EmbedProductListingCommand {
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
    fn should_map_product_event_job_to_embedding_command() {
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();
        let command = command_from_job(DomainJob {
            target_queue: WorkerQueue::ProductListingEmbed,
            idempotency_key: IdempotencyKey::new("product-event:test"),
            ordering_key: OrderingKey::new("product:test"),
            payload: DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id,
                product_listing_id,
            }),
        });
        assert!(
            matches!(command, Ok(EmbedProductListingCommand { event_id: actual, product_listing_id: product }) if actual == event_id && product == product_listing_id)
        );
    }
    #[test]
    fn should_complete_authoritative_missing_title_but_retain_missing_source() {
        assert_eq!(
            JobOutcome::Complete("missing_title"),
            embedding_outcome(EmbedProductListingEventOutcome::MissingTitle)
        );
        assert_eq!(
            JobOutcome::Retry("missing_source"),
            embedding_outcome(EmbedProductListingEventOutcome::ProductListingNotFound)
        );
    }
}

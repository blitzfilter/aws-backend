use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use item_index::IndexItemDocumentRepository;
use lambda_runtime::LambdaEvent;

#[tracing::instrument(skip(_repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    _repository: &impl IndexItemDocumentRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    todo!()
}

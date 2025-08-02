use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use item_write::service::InboundWriteItems;
use lambda_runtime::LambdaEvent;

#[tracing::instrument(skip(_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    _service: &impl InboundWriteItems,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    todo!()
}

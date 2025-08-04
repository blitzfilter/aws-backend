use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};
use aws_sdk_dynamodb::Client;
use common::has::Has;
use lambda_runtime::LambdaEvent;

#[tracing::instrument(skip(_service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    _service: &impl Has<Client>,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    todo!()
}

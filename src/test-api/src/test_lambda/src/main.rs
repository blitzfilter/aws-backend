use lambda_runtime::{LambdaEvent, run, service_fn};
use serde_json::Value;
use test_lambda::handler;

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    run(service_fn(move |event: LambdaEvent<Value>| async move {
        handler(event).await
    }))
    .await
}

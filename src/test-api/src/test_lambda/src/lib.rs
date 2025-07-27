use lambda_runtime::LambdaEvent;
use serde_json::Value;

pub async fn handler(_event: LambdaEvent<Value>) -> Result<String, lambda_runtime::Error> {
    Ok("Hello from test_lambda!".to_string())
}

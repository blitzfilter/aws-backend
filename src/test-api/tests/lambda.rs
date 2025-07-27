use aws_sdk_sqs::primitives::Blob;
use test_api::*;
use test_api_macros::localstack_test;

const TEST_LAMBDA: Lambda = Lambda {
    name: "test_lambda",
    path: "src/test_lambda",
    role: None,
};

#[localstack_test(services = [TEST_LAMBDA])]
async fn should_run_without_errors() {}

#[localstack_test(services = [TEST_LAMBDA])]
async fn should_create_lambda() {
    let functions = get_lambda_client()
        .await
        .list_functions()
        .send()
        .await
        .unwrap()
        .functions
        .unwrap_or_default();

    assert_eq!(1, functions.len());
}

#[localstack_test(services = [TEST_LAMBDA])]
async fn should_invoke_lambda() {
    let response = get_lambda_client()
        .await
        .invoke()
        .function_name(TEST_LAMBDA.name)
        .payload(Blob::new("{}"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        "\"Hello from test_lambda!\"",
        String::from_utf8(response.payload.unwrap().into_inner()).unwrap()
    )
}

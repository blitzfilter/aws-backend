use test_api::*;
use tracing::info;

const SQS_WITH_LAMBDA: SqsWithLambda = SqsWithLambda {
    name: "test_sqs",
    lambda: &Lambda {
        name: "test_lambda",
        path: "src/test_lambda",
        role: None,
    },
    max_batch_size: 1000,
    max_batch_window_seconds: 3,
};

#[localstack_test(services = [SQS_WITH_LAMBDA])]
async fn should_run_without_errors() {}

#[localstack_test(services = [SQS_WITH_LAMBDA])]
async fn should_post_to_sqs() {
    let res = get_sqs_client()
        .await
        .send_message()
        .queue_url(SQS_WITH_LAMBDA.queue_url())
        .message_body("{}")
        .send()
        .await
        .unwrap();
}

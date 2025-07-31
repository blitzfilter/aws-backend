use test_api::*;

const SQS: Sqs = Sqs { name: "test_sqs" };

#[localstack_test(services = [SQS])]
async fn should_run_without_errors() {}

#[localstack_test(services = [SQS])]
async fn should_post_to_sqs() {
    let _ = get_sqs_client()
        .await
        .send_message()
        .queue_url(SQS.queue_url())
        .message_body("{}")
        .send()
        .await
        .unwrap();
}

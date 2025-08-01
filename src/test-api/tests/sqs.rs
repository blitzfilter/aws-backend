use test_api::*;

const SQS: Sqs = Sqs { name: "test_sqs" };

#[localstack_test(services = [SQS])]
async fn should_run_without_errors() {}

#[localstack_test(services = [SQS])]
async fn should_post_to_sqs() {
    let client = get_sqs_client().await;
    let _ = client
        .send_message()
        .queue_url(SQS.queue_url())
        .message_body(r#"{"foo":"bar"}"#)
        .send()
        .await
        .unwrap();

    let res = client
        .receive_message()
        .queue_url(SQS.queue_url())
        .send()
        .await
        .unwrap();

    assert_eq!(
        r#"{"foo":"bar"}"#,
        res.messages.unwrap().remove(0).body.unwrap()
    )
}

#[localstack_test(services = [SQS, Sqs { name: "test_sqs_foo" }])]
async fn should_create_multiple_sqs() {
    let client = get_sqs_client().await;

    let list_qs = client.list_queues().max_results(1000).send().await.unwrap();

    assert_eq!(2, list_qs.queue_urls.unwrap().len())
}

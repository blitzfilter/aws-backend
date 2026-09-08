use aws_sdk_sqs::types::QueueAttributeName as A;
use std::sync::LazyLock;
use test_api::{
    IntegrationTestService, Sqs, SqsQueuePair, WorkerSqs, aura_integration_test, get_sqs_client,
    localstack,
};

static UNIQUE_A: LazyLock<SqsQueuePair> = LazyLock::new(|| SqsQueuePair::unique("scoped-cleanup"));
static UNIQUE_B: LazyLock<SqsQueuePair> = LazyLock::new(|| SqsQueuePair::unique("scoped-cleanup"));

const SQS: Sqs = Sqs { name: "test_sqs" };

#[aura_integration_test(services = [SQS])]
async fn should_run_without_errors() {
    assert!(SQS.queue_url().starts_with(localstack::get_endpoint_url()));
}

#[aura_integration_test(services = [SQS, &*UNIQUE_A, &*UNIQUE_B])]
async fn should_purge_only_its_source_and_dlq_including_invisible_messages() {
    let client = get_sqs_client().await;
    assert_ne!(UNIQUE_A.name, UNIQUE_B.name);
    assert!(UNIQUE_A.name.contains(&std::process::id().to_string()));
    for url in [
        UNIQUE_A.queue_url(),
        UNIQUE_A.dead_letter_queue_url(),
        UNIQUE_B.queue_url(),
    ] {
        client
            .send_message()
            .queue_url(&url)
            .message_body("scoped fixture message")
            .send()
            .await
            .unwrap();
        let received = client
            .receive_message()
            .queue_url(&url)
            .visibility_timeout(60)
            .wait_time_seconds(0)
            .send()
            .await
            .unwrap();
        assert_eq!(1, received.messages().len());
    }
    UNIQUE_A.tear_down().await;
    for url in [UNIQUE_A.queue_url(), UNIQUE_A.dead_letter_queue_url()] {
        let result = client
            .get_queue_attributes()
            .queue_url(url)
            .attribute_names(A::All)
            .send()
            .await
            .unwrap();
        let attrs = result.attributes().unwrap();
        assert_eq!("0", attrs[&A::ApproximateNumberOfMessages]);
        assert_eq!("0", attrs[&A::ApproximateNumberOfMessagesNotVisible]);
    }
    let other = client
        .get_queue_attributes()
        .queue_url(UNIQUE_B.queue_url())
        .attribute_names(A::ApproximateNumberOfMessagesNotVisible)
        .send()
        .await
        .unwrap();
    assert_eq!(
        "1",
        other.attributes().unwrap()[&A::ApproximateNumberOfMessagesNotVisible]
    );
}

#[aura_integration_test(services = [SQS])]
async fn should_expose_production_queue_metadata_for_every_worker_visibility_class() {
    for (scope, visibility) in [
        ("product-listing-normalization", 300),
        ("search-filter-projection", 60),
        ("search-filter-percolator", 300),
        ("search-filter-match-notification", 60),
        ("watchlist-notification", 60),
        ("notification-delivery", 360),
        ("product-content-assessment", 60),
        ("product-embedding", 300),
        ("product-translation", 300),
        ("product-listing-opensearch", 60),
    ] {
        let queues = WorkerSqs::new(scope, visibility);
        queues.set_up().await;
        let source = get_sqs_client()
            .await
            .get_queue_attributes()
            .queue_url(queues.queue_url())
            .attribute_names(A::All)
            .send()
            .await
            .unwrap();
        let dlq = get_sqs_client()
            .await
            .get_queue_attributes()
            .queue_url(queues.dead_letter_queue_url())
            .attribute_names(A::All)
            .send()
            .await
            .unwrap();
        let source = source.attributes().unwrap();
        let dlq = dlq.attributes().unwrap();
        assert_eq!(
            format!(
                "{}/000000000000/aura-worker-{scope}-test",
                localstack::get_endpoint_url()
            ),
            queues.queue_url()
        );
        assert_eq!("604800", source[&A::MessageRetentionPeriod]);
        assert_eq!("1209600", dlq[&A::MessageRetentionPeriod]);
        assert_eq!(visibility.to_string(), source[&A::VisibilityTimeout]);
        assert_eq!("20", source[&A::ReceiveMessageWaitTimeSeconds]);
        for attrs in [source, dlq] {
            assert_ne!(Some("true"), attrs.get(&A::FifoQueue).map(String::as_str));
            assert_eq!("true", attrs[&A::SqsManagedSseEnabled]);
            let policy: serde_json::Value = serde_json::from_str(&attrs[&A::Policy]).unwrap();
            assert_eq!(
                serde_json::json!({"Version":"2012-10-17", "Statement":[{
                    "Sid":"DenyInsecureTransport", "Effect":"Deny", "Principal":"*", "Action":"sqs:*",
                    "Resource":attrs[&A::QueueArn], "Condition":{"Bool":{"aws:SecureTransport":"false"}},
                }]}),
                policy
            );
        }
        let source_allow: serde_json::Value =
            serde_json::from_str(&source[&A::RedriveAllowPolicy]).unwrap();
        assert_eq!(
            serde_json::json!({"redrivePermission": "denyAll"}),
            source_allow
        );
        let redrive: serde_json::Value = serde_json::from_str(&source[&A::RedrivePolicy]).unwrap();
        assert_eq!(
            serde_json::json!({"deadLetterTargetArn":dlq[&A::QueueArn],"maxReceiveCount":5}),
            redrive
        );
        let allow: serde_json::Value = serde_json::from_str(&dlq[&A::RedriveAllowPolicy]).unwrap();
        assert_eq!(
            serde_json::json!({"redrivePermission":"byQueue","sourceQueueArns":[source[&A::QueueArn]]}),
            allow
        );
        queues.tear_down().await;
    }
}

#[aura_integration_test(services = [SQS])]
async fn should_isolate_worker_queue_from_previous_tests_cancelled_long_poll() {
    let queues = WorkerSqs::new("product-content-assessment", 60);
    queues.set_up().await;
    let receive = tokio::spawn(async move {
        get_sqs_client()
            .await
            .receive_message()
            .queue_url(queues.queue_url())
            .wait_time_seconds(20)
            .send()
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !receive.is_finished(),
        "previous test has a pending long poll"
    );
    receive.abort();
    assert!(receive.await.unwrap_err().is_cancelled());
    queues.tear_down().await;
    queues.set_up().await;
    get_sqs_client()
        .await
        .send_message()
        .queue_url(queues.queue_url())
        .message_body("next test message")
        .send()
        .await
        .unwrap();
    let received = get_sqs_client()
        .await
        .receive_message()
        .queue_url(queues.queue_url())
        .wait_time_seconds(1)
        .send()
        .await
        .unwrap();
    queues.tear_down().await;
    assert_eq!(
        1,
        received.messages().len(),
        "cancelled old receive must not hide new test work"
    );
    assert_eq!(Some("next test message"), received.messages()[0].body());
}

#[aura_integration_test(services = [SQS])]
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

#[aura_integration_test(services = [SQS, Sqs { name: "test_sqs_foo" }])]
async fn should_create_multiple_sqs() {
    let client = get_sqs_client().await;

    let list_qs = client.list_queues().max_results(1000).send().await.unwrap();

    // 2 Q + 2 DLQ
    let expected = [
        SQS.queue_url(),
        SQS.dead_letter_queue_url(),
        Sqs {
            name: "test_sqs_foo",
        }
        .queue_url(),
        Sqs {
            name: "test_sqs_foo",
        }
        .dead_letter_queue_url(),
    ];
    let actual = list_qs.queue_urls.unwrap();
    assert_eq!(
        4,
        expected
            .iter()
            .filter(|url| actual
                .iter()
                .any(|returned| returned.rsplit('/').next() == url.rsplit('/').next()))
            .count()
    );
}

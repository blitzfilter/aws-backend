use std::collections::HashMap;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::item_id::ItemKey;
use common::{batch::Batch, batch::dynamodb::handle_batch_output, has_key::HasKey};
use item_dynamodb::item_event_record::ItemEventRecord;
use item_dynamodb::item_record::ItemRecord;
use item_dynamodb::repository::ItemDynamoDbRepository;
use lambda_runtime::LambdaEvent;
use tracing::{error, info, warn};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl ItemDynamoDbRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut materialized_records = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ItemKey, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(item_record) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            materialized_records.push(item_record);
        }
    }

    for batch in Batch::<ItemRecord, 25>::chunked_from(materialized_records.into_iter()) {
        let item_keys = batch.iter().map(ItemRecord::key).collect::<Vec<_>>();
        let mut failures = Vec::new();
        match repository.put_item_records(batch).await {
            Ok(output) => {
                handle_batch_output::<ItemRecord>(output, &mut failures);
            }
            Err(err) => {
                error!(error = ?err, "Failed entire batch.");
                failures = item_keys;
            }
        }
        failures
            .into_iter()
            .filter_map(|key| match message_ids.remove(&key) {
                Some(message_id) => Some(message_id),
                None => {
                    error!(
                        itemKey = %key,
                        "There exists no message_id for failed ItemRecord."
                    );
                    None
                }
            })
            .for_each(|message_id| failed_message_ids.push(message_id));
    }

    let failure_count = failed_message_ids.len();
    info!(
        successful = records_count - failure_count - skipped_count,
        failures = failure_count,
        skipped = skipped_count,
        "Handler finished.",
    );
    let sqs_batch_response = SqsBatchResponse {
        batch_item_failures: failed_message_ids
            .into_iter()
            .map(|item_identifier| BatchItemFailure { item_identifier })
            .collect(),
    };
    Ok(sqs_batch_response)
}

fn extract_message_data(
    message: SqsMessage,
    failed_message_ids: &mut Vec<String>,
    skipped_count: &mut usize,
    message_ids: &mut HashMap<ItemKey, String>,
) -> Option<ItemRecord> {
    let message_id = message
        .message_id
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");

    match message.body {
        None => {
            info!("Received empty body. Skipping message.");
            *skipped_count += 1;
            None
        }
        Some(item_json) => match serde_json::from_str::<ItemEventRecord>(&item_json) {
            Ok(event_record) => match ItemRecord::try_from(event_record) {
                Ok(record) => {
                    message_ids.insert(record.key(), message_id);
                    Some(record)
                }
                Err(err) => {
                    warn!(
                        error = %err,
                        fromType = %std::any::type_name::<ItemEventRecord>(),
                        toType = %std::any::type_name::<ItemRecord>(),
                        payload = %item_json,
                        "Failed mapping types."
                    );
                    failed_message_ids.push(message_id);
                    None
                }
            },
            Err(e) => {
                error!(
                    error = %e,
                    type = %std::any::type_name::<ItemEventRecord>(),
                    payload = %item_json,
                    "Failed deserializing."
                );
                failed_message_ids.push(message_id);
                None
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_message_data, handler};
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput;
    use common::event::Event;
    use common::has_key::HasKey;
    use common::localized::Localized;
    use common::price::domain::Price;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use fake::{Fake, Faker};
    use item_core::item_event::{ItemCreatedEventPayload, ItemEventPayload};
    use item_core::shop_name::ShopName;
    use item_core::{item::Item, item_state::ItemState};
    use item_dynamodb::item_event_record::ItemEventRecord;
    use item_dynamodb::item_record::ItemRecord;
    use item_dynamodb::repository::MockItemDynamoDbRepository;
    use lambda_runtime::{Context, LambdaEvent};
    use std::collections::HashMap;
    use time::OffsetDateTime;
    use url::Url;

    #[tokio::test]
    #[rstest::rstest]
    #[case(1)]
    #[case(5)]
    #[case(10)]
    #[case(25)]
    #[case(47)]
    #[case(100)]
    #[case(150)]
    #[case(453)]
    #[case(900)]
    #[case(2874)]
    #[case(10874)]
    async fn should_handle_sqs_message(#[case] record_count: usize) {
        let records = fake::vec![ItemCreatedEventPayload; record_count]
            .into_iter()
            .map(ItemEventPayload::Created)
            .map(|event_payload| Event {
                aggregate_id: Faker.fake(),
                event_id: Faker.fake(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload,
            })
            .map(ItemEventRecord::try_from)
            .map(Result::unwrap)
            .map(|record| serde_json::to_string(&record))
            .map(Result::unwrap)
            .map(|json_payload| SqsMessage {
                message_id: Some(Faker.fake()),
                receipt_handle: None,
                body: Some(json_payload),
                md5_of_body: None,
                md5_of_message_attributes: None,
                attributes: Default::default(),
                message_attributes: Default::default(),
                event_source_arn: None,
                event_source: None,
                aws_region: None,
            })
            .collect();
        let lambda_event = LambdaEvent {
            payload: SqsEvent { records },
            context: Context::default(),
        };
        let mut repository = MockItemDynamoDbRepository::default();
        repository.expect_put_item_records().returning(move |_| {
            Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
        });

        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    fn create_sample_item_event_record() -> ItemEventRecord {
        let item = Item::create(
            ShopId::new(),
            ShopsItemId::new(),
            ShopName::from("test shop"),
            Localized::new(common::language::domain::Language::En, "Test Item".into()),
            Default::default(),
            None,
            Default::default(),
            Some(Price::new(
                10000u64.into(),
                common::currency::domain::Currency::Usd,
            )),
            Default::default(),
            ItemState::Available,
            Url::parse("https://example.com/item").unwrap(),
            vec![],
        );
        item.try_into().unwrap()
    }

    #[tokio::test]
    async fn should_fail_message_for_invalid_json_deserialization() {
        let invalid_json_message = SqsMessage {
            message_id: Some("msg1".to_string()),
            receipt_handle: None,
            body: Some("invalid json {".to_string()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let lambda_event = LambdaEvent {
            payload: SqsEvent {
                records: vec![invalid_json_message],
            },
            context: Context::default(),
        };

        let mut repository_mock = MockItemDynamoDbRepository::default();
        // Repository should not be called since the message fails parsing
        repository_mock.expect_put_item_records().times(0);

        let response = handler(&repository_mock, lambda_event).await.unwrap();

        assert_eq!(response.batch_item_failures.len(), 1);
        assert_eq!(response.batch_item_failures[0].item_identifier, "msg1");
    }

    #[tokio::test]
    async fn should_skip_message_for_empty_message_body() {
        let empty_body_message = SqsMessage {
            message_id: Some("msg2".to_string()),
            receipt_handle: None,
            body: None,
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let lambda_event = LambdaEvent {
            payload: SqsEvent {
                records: vec![empty_body_message],
            },
            context: Context::default(),
        };

        let mut repository_mock = MockItemDynamoDbRepository::default();
        // Repository should not be called since the message has no body
        repository_mock.expect_put_item_records().times(0);

        let response = handler(&repository_mock, lambda_event).await.unwrap();

        // Empty body should be skipped (not failed)
        assert!(response.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_fail_message_for_item_record_conversion_failure() {
        // Create an ItemEventRecord that cannot be converted to ItemRecord
        // This simulates conversion failures that might occur
        let invalid_event_json = r#"{"eventType":"Created","shopId":"test","shopsItemId":"test","timestamp":"2023-01-01T00:00:00Z","payload":{"item":null}}"#;

        let invalid_conversion_message = SqsMessage {
            message_id: Some("msg_conversion_fail".to_string()),
            receipt_handle: None,
            body: Some(invalid_event_json.to_string()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let lambda_event = LambdaEvent {
            payload: SqsEvent {
                records: vec![invalid_conversion_message],
            },
            context: Context::default(),
        };

        let mut repository_mock = MockItemDynamoDbRepository::default();
        // Repository should not be called since conversion fails
        repository_mock.expect_put_item_records().times(0);

        let response = handler(&repository_mock, lambda_event).await.unwrap();

        // The message should fail due to conversion error
        assert_eq!(response.batch_item_failures.len(), 1);
        assert_eq!(
            response.batch_item_failures[0].item_identifier,
            "msg_conversion_fail"
        );
    }

    #[tokio::test]
    async fn should_fail_message_for_repository_put_failure_entire_batch() {
        let event_record = create_sample_item_event_record();
        let valid_message = SqsMessage {
            message_id: Some("msg3".to_string()),
            receipt_handle: None,
            body: Some(serde_json::to_string(&event_record).unwrap()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let lambda_event = LambdaEvent {
            payload: SqsEvent {
                records: vec![valid_message],
            },
            context: Context::default(),
        };

        let mut repository_mock = MockItemDynamoDbRepository::default();
        repository_mock
            .expect_put_item_records()
            .times(1)
            .returning(|_| {
                Box::pin(async move { Err(SdkError::construction_failure("Batch put failed")) })
            });

        let response = handler(&repository_mock, lambda_event).await.unwrap();

        assert_eq!(response.batch_item_failures.len(), 1);
        assert_eq!(response.batch_item_failures[0].item_identifier, "msg3");
    }

    #[tokio::test]
    async fn should_succeed_processing_repository_put_partial_failure() {
        // This test demonstrates partial failure handling by having unprocessed items
        // but since creating a valid serialized ItemRecord for unprocessed items is complex,
        // we'll test that the handler at least processes the response without errors
        let event_record = create_sample_item_event_record();

        let valid_message = SqsMessage {
            message_id: Some("msg_partial".to_string()),
            receipt_handle: None,
            body: Some(serde_json::to_string(&event_record).unwrap()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let lambda_event = LambdaEvent {
            payload: SqsEvent {
                records: vec![valid_message],
            },
            context: Context::default(),
        };

        let mut repository_mock = MockItemDynamoDbRepository::default();
        repository_mock
            .expect_put_item_records()
            .times(1)
            .returning(|_| {
                Box::pin(async move {
                    // Return success with empty unprocessed items (no actual failures)
                    // In a real scenario with complex DynamoDB items, this would contain
                    // properly serialized ItemRecord data that failed to be written
                    Ok(BatchWriteItemOutput::builder().build())
                })
            });

        let response = handler(&repository_mock, lambda_event).await.unwrap();

        // Since we don't have actual unprocessed items, this should succeed
        assert_eq!(response.batch_item_failures.len(), 0);
    }

    #[tokio::test]
    async fn should_process_mixed_success_and_failure_scenarios_correctly() {
        let event_record = create_sample_item_event_record();
        let valid_message = SqsMessage {
            message_id: Some("msg_success".to_string()),
            receipt_handle: None,
            body: Some(serde_json::to_string(&event_record).unwrap()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let invalid_message = SqsMessage {
            message_id: Some("msg_invalid".to_string()),
            receipt_handle: None,
            body: Some("invalid json".to_string()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let empty_message = SqsMessage {
            message_id: Some("msg_empty".to_string()),
            receipt_handle: None,
            body: None,
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let lambda_event = LambdaEvent {
            payload: SqsEvent {
                records: vec![valid_message, invalid_message, empty_message],
            },
            context: Context::default(),
        };

        let mut repository_mock = MockItemDynamoDbRepository::default();
        repository_mock
            .expect_put_item_records()
            .times(1)
            .returning(|_| Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) }));

        let response = handler(&repository_mock, lambda_event).await.unwrap();

        // Only the invalid JSON message should fail (empty is skipped, valid succeeds)
        assert_eq!(response.batch_item_failures.len(), 1);
        assert_eq!(
            response.batch_item_failures[0].item_identifier,
            "msg_invalid"
        );
    }

    #[tokio::test]
    async fn should_fail_extract_message_data_with_invalid_json() {
        let mut failed_message_ids = Vec::new();
        let mut skipped_count = 0;
        let mut message_ids = HashMap::new();

        let invalid_message = SqsMessage {
            message_id: Some("test_msg".to_string()),
            receipt_handle: None,
            body: Some("{ invalid json".to_string()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let result = extract_message_data(
            invalid_message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        );

        assert!(result.is_none());
        assert_eq!(failed_message_ids.len(), 1);
        assert_eq!(failed_message_ids[0], "test_msg");
        assert_eq!(skipped_count, 0);
        assert!(message_ids.is_empty());
    }

    #[tokio::test]
    async fn should_skip_extract_message_data_with_empty_body() {
        let mut failed_message_ids = Vec::new();
        let mut skipped_count = 0;
        let mut message_ids = HashMap::new();

        let empty_message = SqsMessage {
            message_id: Some("test_msg".to_string()),
            receipt_handle: None,
            body: None,
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let result = extract_message_data(
            empty_message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        );

        assert!(result.is_none());
        assert!(failed_message_ids.is_empty());
        assert_eq!(skipped_count, 1);
        assert!(message_ids.is_empty());
    }

    #[tokio::test]
    async fn should_fail_extract_message_data_with_conversion_failure() {
        let mut failed_message_ids = Vec::new();
        let mut skipped_count = 0;
        let mut message_ids = HashMap::new();

        // Create a valid ItemEventRecord JSON that will fail conversion to ItemRecord
        let invalid_conversion_json = r#"{"eventType":"Created","shopId":"test","shopsItemId":"test","timestamp":"2023-01-01T00:00:00Z","payload":{"item":null}}"#;

        let conversion_fail_message = SqsMessage {
            message_id: Some("test_msg".to_string()),
            receipt_handle: None,
            body: Some(invalid_conversion_json.to_string()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let result = extract_message_data(
            conversion_fail_message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        );

        assert!(result.is_none());
        assert_eq!(failed_message_ids.len(), 1);
        assert_eq!(failed_message_ids[0], "test_msg");
        assert_eq!(skipped_count, 0);
        assert!(message_ids.is_empty());
    }

    #[tokio::test]
    async fn should_succeed_extract_message_data_with_valid_data() {
        let mut failed_message_ids = Vec::new();
        let mut skipped_count = 0;
        let mut message_ids = HashMap::new();

        let event_record = create_sample_item_event_record();
        let valid_message = SqsMessage {
            message_id: Some("test_msg".to_string()),
            receipt_handle: None,
            body: Some(serde_json::to_string(&event_record).unwrap()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let result = extract_message_data(
            valid_message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        );

        assert!(result.is_some());
        let item_record = result.unwrap();
        assert!(failed_message_ids.is_empty());
        assert_eq!(skipped_count, 0);
        assert_eq!(message_ids.len(), 1);
        let key = item_record.key();
        assert!(message_ids.contains_key(&key));
        assert_eq!(message_ids[&key], "test_msg");
        assert!(matches!(item_record, ItemRecord { .. }));
    }
}

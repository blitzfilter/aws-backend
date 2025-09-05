use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::item_id::ItemKey;
use common::{batch::Batch, batch::dynamodb::handle_batch_output, has_key::HasKey};
use item_dynamodb::item_event_record::ItemEventRecord;
use item_dynamodb::item_record::ItemRecord;
use item_dynamodb::repository::ItemDynamoDbRepository;
use item_lambda_common::extract_item_event_record;
use lambda_runtime::LambdaEvent;
use std::collections::HashMap;
use tracing::{error, info};

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
        .clone()
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");
    let item_event_record = extract_item_event_record(message, failed_message_ids, skipped_count)?;
    match ItemRecord::try_from(item_event_record) {
        Ok(record) => {
            message_ids.insert(record.key(), message_id);
            Some(record)
        }
        Err(err) => {
            error!(
                error = %err,
                fromType = %std::any::type_name::<ItemEventRecord>(),
                toType = %std::any::type_name::<ItemRecord>(),
                "Failed mapping types."
            );
            failed_message_ids.push(message_id);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput;
    use common::event::Event;
    use common::has_key::HasKey;
    use common::item_id::ItemKey;
    use fake::{Fake, Faker};
    use item_core::item_event::{ItemCreatedEventPayload, ItemEventPayload};
    use item_dynamodb::item_event_record::ItemEventRecord;
    use item_dynamodb::repository::MockItemDynamoDbRepository;
    use lambda_runtime::{Context, LambdaEvent};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;
    use test_api::mk_partial_put_batch_failure;
    use time::OffsetDateTime;
    use uuid::Uuid;

    fn mk_event_bridge_payload(item_event_record: &ItemEventRecord) -> String {
        let event = EventBridgeEvent {
            version: None,
            id: None,
            detail_type: "foo".to_string(),
            source: "bar".to_string(),
            account: None,
            time: None,
            region: None,
            resources: None,
            detail: EventRecord {
                aws_region: "eu-central-1".to_string(),
                change: StreamRecord {
                    approximate_creation_date_time: SystemTime::now().into(),
                    keys: Default::default(),
                    new_image: serde_dynamo::to_item(item_event_record).unwrap(),
                    old_image: Default::default(),
                    sequence_number: None,
                    size_bytes: 42,
                    stream_view_type: None,
                },
                event_id: Uuid::new_v4().to_string(),
                event_name: "INSERT".to_string(),
                event_source: None,
                event_version: None,
                event_source_arn: None,
                user_identity: None,
                record_format: None,
                table_name: None,
            },
        };
        serde_json::to_string(&event).unwrap()
    }

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
            .map(|record| SqsMessage {
                message_id: Some(Faker.fake()),
                receipt_handle: None,
                body: Some(mk_event_bridge_payload(&record)),
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
    async fn should_respond_with_partial_failures_when_ddb_full_batch_failure(
        #[case] record_count: usize,
    ) {
        let mut message_ids = HashMap::with_capacity(record_count);
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
            .map(|record| {
                let uuid = Uuid::new_v4().to_string();
                message_ids.insert(record.key(), uuid.clone());
                SqsMessage {
                    message_id: Some(uuid),
                    receipt_handle: None,
                    body: Some(mk_event_bridge_payload(&record)),
                    md5_of_body: None,
                    md5_of_message_attributes: None,
                    attributes: Default::default(),
                    message_attributes: Default::default(),
                    event_source_arn: None,
                    event_source: None,
                    aws_region: None,
                }
            })
            .collect();
        let lambda_event = LambdaEvent {
            payload: SqsEvent { records },
            context: Context::default(),
        };
        let failed_keys: Arc<Mutex<Vec<ItemKey>>> = Arc::new(Mutex::new(vec![]));
        let failed_keys_clone = failed_keys.clone();
        let mut repository = MockItemDynamoDbRepository::default();
        repository
            .expect_put_item_records()
            .returning(move |batch| {
                if Faker.fake() {
                    batch
                        .into_iter()
                        .map(|record| record.key())
                        .for_each(|key| failed_keys_clone.lock().unwrap().push(key));
                    Box::pin(
                        async move { Err(SdkError::construction_failure("Something went wrong")) },
                    )
                } else {
                    Box::pin(async move { Ok(BatchWriteItemOutput::builder().build()) })
                }
            });
        let mut actual_failed_message_ids = handler(&repository, lambda_event)
            .await
            .unwrap()
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual_failed_message_ids.sort();
        let mut expected_failed_message_ids = failed_keys
            .lock()
            .unwrap()
            .iter()
            .map(|key| message_ids.remove(key))
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(9, 10)]
    #[case(0, 25)]
    #[case(47, 47)]
    #[case(100, 100)]
    #[case(0, 150)]
    #[case(234, 453)]
    #[case(773, 900)]
    #[case(299, 2874)]
    #[case(77, 10874)]
    async fn should_respond_with_partial_failures_when_ddb_partial_batch_failure(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let mut message_ids = HashMap::with_capacity(record_count);
        let expected_failures = message_ids
            .keys()
            .take(failure_count)
            .cloned()
            .collect::<Vec<_>>();
        let expected_failures_clone = expected_failures.clone();
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
            .map(|record| {
                let uuid = Uuid::new_v4().to_string();
                message_ids.insert(record.key(), uuid.clone());
                SqsMessage {
                    message_id: Some(uuid),
                    receipt_handle: None,
                    body: Some(mk_event_bridge_payload(&record)),
                    md5_of_body: None,
                    md5_of_message_attributes: None,
                    attributes: Default::default(),
                    message_attributes: Default::default(),
                    event_source_arn: None,
                    event_source: None,
                    aws_region: None,
                }
            })
            .collect();
        let lambda_event = LambdaEvent {
            payload: SqsEvent { records },
            context: Context::default(),
        };
        let mut repository = MockItemDynamoDbRepository::default();
        repository
            .expect_put_item_records()
            .returning(move |batch| {
                let unprocessed = batch
                    .into_iter()
                    .filter(|item_record| expected_failures_clone.contains(&item_record.key()))
                    .collect();
                Box::pin(async move {
                    Ok(BatchWriteItemOutput::builder()
                        .set_unprocessed_items(mk_partial_put_batch_failure("table_1", unprocessed))
                        .build())
                })
            });
        let mut actual_failed_message_ids = handler(&repository, lambda_event)
            .await
            .unwrap()
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual_failed_message_ids.sort();
        let mut expected_failed_message_ids = expected_failures
            .into_iter()
            .map(|key| message_ids.remove(&key))
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }
}

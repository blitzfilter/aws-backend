use std::collections::HashMap;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::has_key::HasKey;
use common::item_id::ItemKey;
use item_dynamodb::item_update_record::ItemRecordUpdate;
use item_dynamodb::repository::ItemDynamoDbRepository;
use item_lambda_common::extract_item_event_record;
use lambda_runtime::LambdaEvent;
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
    let mut updates = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ItemKey, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(update) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            updates.push(update);
        }
    }

    for (key, update) in updates {
        let update_res = repository
            .update_item_record(&key.shop_id, &key.shops_item_id, update)
            .await;
        if let Err(err) = update_res {
            error!(error = ?err, itemKey = %key, "Failed update.");
            match message_ids.remove(&key) {
                Some(message_id) => failed_message_ids.push(message_id),
                None => {
                    error!(
                        itemKey = %key,
                        "There exists no message_id for failed ItemRecord."
                    );
                }
            }
        }
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
) -> Option<(ItemKey, ItemRecordUpdate)> {
    let message_id = message
        .message_id
        .clone()
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");
    let item_event_record = extract_item_event_record(message, failed_message_ids, skipped_count)?;
    let key = item_event_record.key();
    let update_record = ItemRecordUpdate::from(item_event_record);
    message_ids.insert(key.clone(), message_id);
    Some((key, update_record))
}

#[cfg(test)]
mod tests {
    use super::handler;
    use aws_lambda_events::dynamodb::{EventRecord, StreamRecord};
    use aws_lambda_events::eventbridge::EventBridgeEvent;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use aws_sdk_dynamodb::error::SdkError;
    use aws_sdk_dynamodb::operation::update_item::UpdateItemOutput;
    use fake::{Fake, Faker};
    use item_core::item_event::{ItemCommonEventPayload, ItemEvent};
    use item_dynamodb::item_event_record::ItemEventRecord;
    use item_dynamodb::repository::MockItemDynamoDbRepository;
    use lambda_runtime::{Context, LambdaEvent};
    use std::time::SystemTime;
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
        let records = fake::vec![ItemEvent; record_count]
            .into_iter()
            .map(ItemEventRecord::try_from)
            .map(Result::unwrap)
            .map(|item_event_record| SqsMessage {
                message_id: Some(Faker.fake()),
                receipt_handle: None,
                body: Some(mk_event_bridge_payload(&item_event_record)),
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
        repository
            .expect_update_item_record()
            .returning(move |_, _, _| {
                Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
            });

        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty());
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case(0, 1)]
    #[case(1, 1)]
    #[case(2, 5)]
    #[case(7, 10)]
    #[case(24, 25)]
    #[case(0, 47)]
    #[case(98, 100)]
    #[case(1, 150)]
    #[case(0, 453)]
    #[case(0, 900)]
    #[case(2874, 2874)]
    #[case(874, 10874)]
    async fn should_respond_with_partial_failures(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let events = fake::vec![ItemEvent; record_count];
        let expected_failed_events = events
            .clone()
            .into_iter()
            .take(failure_count)
            .collect::<Vec<_>>();
        let mut expected_failed_message_ids = Vec::with_capacity(failure_count);
        let records = events
            .into_iter()
            .map(ItemEventRecord::try_from)
            .map(Result::unwrap)
            .map(|event_record| {
                let message_id = Uuid::new_v4().to_string();
                if expected_failed_events.iter().any(|event| {
                    event.payload.shop_id() == &event_record.shop_id
                        && event.payload.shops_item_id() == &event_record.shops_item_id
                }) {
                    expected_failed_message_ids.push(message_id.clone());
                }
                SqsMessage {
                    message_id: Some(message_id),
                    receipt_handle: None,
                    body: Some(mk_event_bridge_payload(&event_record)),
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
            .expect_update_item_record()
            .returning(move |shop_id, shops_item_id, _| {
                if expected_failed_events.iter().any(|event| {
                    event.payload.shop_id() == shop_id
                        && event.payload.shops_item_id() == shops_item_id
                }) {
                    Box::pin(
                        async move { Err(SdkError::construction_failure("Something went wrong.")) },
                    )
                } else {
                    Box::pin(async move { Ok(UpdateItemOutput::builder().build()) })
                }
            });

        expected_failed_message_ids.sort();
        let mut actual_failed_message_ids = handler(&repository, lambda_event)
            .await
            .unwrap()
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }
}

use std::collections::HashMap;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::item_id::ItemId;
use item_dynamodb::item_event_record::ItemEventRecord;
use common::opensearch::bulk_response::{BulkItemResult, BulkResponse};
use item_opensearch::item_update_document::ItemUpdateDocument;
use item_opensearch::repository::ItemOpenSearchRepository;
use lambda_runtime::LambdaEvent;
use tracing::{error, info, warn};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl ItemOpenSearchRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut update_documents = HashMap::with_capacity(records_count);
    let mut message_ids: HashMap<ItemId, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some((item_id, item_document)) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            update_documents.insert(item_id, item_document);
        }
    }

    let result = repository.update_item_documents(update_documents).await;
    match result {
        Ok(response) => handle_bulk_response(response, &mut failed_message_ids, &mut message_ids),
        Err(err) => {
            error!(error = ?err, "Failed entire batch.");
            failed_message_ids.extend(message_ids.into_values());
        }
    };

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
    message_ids: &mut HashMap<ItemId, String>,
) -> Option<(ItemId, ItemUpdateDocument)> {
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
            Ok(event_record) => {
                let item_id = event_record.item_id;
                let update_document = ItemUpdateDocument::from(event_record);
                message_ids.insert(item_id, message_id);
                Some((item_id, update_document))
            }
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

fn handle_bulk_response(
    response: BulkResponse,
    failed_message_ids: &mut Vec<String>,
    message_ids: &mut HashMap<ItemId, String>,
) {
    if response.errors {
        let failures = response
            .items
            .into_iter()
            .filter_map(|bulk_item_result| match bulk_item_result {
                BulkItemResult::Update { update } => Some(update),
                other => {
                    error!(actual = ?other, "Expected BulkItemResult::Update.");
                    None
                }
            })
            .filter(|bulk_op_result| bulk_op_result.is_err());

        for failure in failures {
            warn!(
                index = failure.index,
                itemId = failure.id,
                status = failure.status,
                error = ?failure.error,
                "Failed creating item in OpenSearch."
            );
            match ItemId::try_from(failure.id.as_str()) {
                Ok(item_id) => match message_ids.remove(&item_id) {
                    Some(message_id) => {
                        failed_message_ids.push(message_id);
                    }
                    None => {
                        error!(
                            index = failure.index,
                            itemId = failure.id,
                            "Failed re-mapping item-id to message-id. Cannot retry."
                        );
                    }
                },
                Err(err) => {
                    error!(
                        index = failure.index,
                        itemId = failure.id,
                        error = %err,
                        payload = ?failure,
                        "Failed parsing '_id' from OpenSearch-Response as 'ItemId'. Cannot retry."
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::event::Event;
    use fake::Fake;
    use fake::Faker;
    use item_core::item_event::ItemEvent;
    use item_core::item_event::{ItemCreatedEventPayload, ItemEventPayload};
    use item_dynamodb::item_event_record::ItemEventRecord;
    use common::opensearch::bulk_response::{BulkError, BulkResponse};
    use common::opensearch::bulk_response::BulkItemResult;
    use common::opensearch::bulk_response::BulkOpResult;
    use item_opensearch::repository::MockItemOpenSearchRepository;
    use lambda_runtime::LambdaEvent;
    use std::collections::HashMap;
    use time::OffsetDateTime;
    use uuid::Uuid;

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
    async fn should_handle_message(#[case] record_count: usize) {
        let mut repository = MockItemOpenSearchRepository::default();
        repository.expect_update_item_documents().return_once(|_| {
            Box::pin(async move {
                Ok(BulkResponse {
                    took: 500,
                    errors: false,
                    items: vec![],
                })
            })
        });

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
        let lambda_event: LambdaEvent<SqsEvent> = LambdaEvent {
            payload: SqsEvent { records },
            context: Default::default(),
        };

        let actual = handler(&repository, lambda_event).await.unwrap();
        assert!(actual.batch_item_failures.is_empty())
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
    async fn should_respond_with_partial_failures_when_opensearch_partial_bulk_failure(
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
        let records = fake::vec![ItemEvent; record_count]
            .into_iter()
            .map(ItemEventRecord::try_from)
            .map(Result::unwrap)
            .map(|record| {
                let uuid = Uuid::new_v4().to_string();
                message_ids.insert(record.item_id, uuid.clone());
                SqsMessage {
                    message_id: Some(uuid),
                    receipt_handle: None,
                    body: Some(serde_json::to_string(&record).unwrap()),
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
            context: Default::default(),
        };
        let mut repository = MockItemOpenSearchRepository::default();
        repository
            .expect_update_item_documents()
            .return_once(move |batch| {
                let failures: Vec<_> = batch
                    .iter()
                    .filter(|(item_id, _)| expected_failures_clone.contains(item_id))
                    .map(|(item_id, _)| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: item_id.to_string(),
                            version: Some(2),
                            result: "not updated".to_string(),
                            status: 409,
                            error: Some(BulkError {
                                error_type: "boop".to_string(),
                                reason: "[items][3]: version conflict, document doesn't exist"
                                    .to_string(),
                                index_uuid: Some(Uuid::new_v4().to_string()),
                                shard: Some("shard-1".to_string()),
                                index: Some(index),
                                extra: Default::default(),
                            }),
                        }
                    })
                    .map(|update| BulkItemResult::Update { update })
                    .collect();

                let successes: Vec<_> = batch
                    .iter()
                    .filter(|(item_id, _)| !expected_failures_clone.contains(item_id))
                    .map(|(item_id, _)| {
                        let index: String = Faker.fake();
                        BulkOpResult {
                            index: index.clone(),
                            id: item_id.to_string(),
                            version: Some(2),
                            result: "updated".to_string(),
                            status: 200,
                            error: None,
                        }
                    })
                    .map(|update| BulkItemResult::Update { update })
                    .collect();

                Box::pin(async move {
                    Ok(BulkResponse {
                        took: 500,
                        errors: true,
                        items: [successes, failures].concat(),
                    })
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
            .map(|item_id| message_ids.remove(&item_id))
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();

        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }
}

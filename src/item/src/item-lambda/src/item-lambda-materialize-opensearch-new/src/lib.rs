use std::collections::HashMap;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::item_id::ItemId;
use item_core::{item::document::ItemDocument, item_event::record::ItemEventRecord};
use item_index::{
    IndexItemDocumentRepository,
    bulk::{BulkItemResult, BulkResponse},
};
use lambda_runtime::LambdaEvent;
use tracing::{error, info, warn};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl IndexItemDocumentRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut materialized_documents = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ItemId, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(item_document) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            materialized_documents.push(item_document);
        }
    }

    let result = repository
        .create_item_documents(materialized_documents)
        .await;
    match result {
        Ok(response) => handle_bulk_response(response, &mut failed_message_ids, &mut message_ids),
        Err(err) => {
            error!(error = ?err, "Failed entire batch.");
            failed_message_ids.extend(message_ids.into_values());
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
    message_ids: &mut HashMap<ItemId, String>,
) -> Option<ItemDocument> {
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
            Ok(event_record) => match ItemDocument::try_from(event_record) {
                Ok(document) => {
                    message_ids.insert(document._id(), message_id);
                    Some(document)
                }
                Err(err) => {
                    warn!(
                        error = %err,
                        fromType = %std::any::type_name::<ItemEventRecord>(),
                        toType = %std::any::type_name::<ItemDocument>(),
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
                BulkItemResult::Create { create } => Some(create),
                other => {
                    error!(actual = ?other, "Expected BulkItemResult::Create.");
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

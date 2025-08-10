use std::collections::HashMap;

use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::item_id::ItemKey;
use common::{batch::Batch, has::HasKey};
use item_core::{item::record::ItemRecord, item_event::record::ItemEventRecord};
use item_write::repository::PersistItemRepository;
use item_write::service::handle_batch_output;
use lambda_runtime::LambdaEvent;
use tracing::{error, info, warn};

#[tracing::instrument(skip(repository, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    repository: &impl PersistItemRepository,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut materialized_records = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ItemKey, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(item_recor) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            materialized_records.push(item_recor);
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

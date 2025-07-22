use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use aws_sdk_dynamodb::Client;
use common::item_id::ItemKey;
use item_core::item::command::CreateItemCommand;
use item_core::item::command_data::CreateItemCommandData;
use item_write::service::InboundWriteItems;
use lambda_runtime::LambdaEvent;
use std::collections::HashMap;
use tracing::{error, info};

#[tracing::instrument(skip(client, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    client: &Client,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut commands = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ItemKey, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(command) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            commands.push(CreateItemCommand::from(command));
        }
    }

    let failed_command_keys = client
        .handle_create_items(commands)
        .await
        .err()
        .unwrap_or_default();
    for failed_command_key in failed_command_keys {
        let message_id = message_ids.remove(&failed_command_key);
        match message_id {
            Some(message_id) => failed_message_ids.push(message_id),
            None => {
                error!(
                    itemKey = failed_command_key.to_string(),
                    "There exists no message_id for a failed command."
                );
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

#[tracing::instrument(
    skip(message, failed_message_ids, skipped_count, message_ids),
    fields(messageId = %message.message_id.as_ref().expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it."))
)]
fn extract_message_data(
    message: SqsMessage,
    failed_message_ids: &mut Vec<String>,
    skipped_count: &mut usize,
    message_ids: &mut HashMap<ItemKey, String>,
) -> Option<CreateItemCommandData> {
    let message_id = message
        .message_id
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");

    match message.body {
        None => {
            info!("Received empty body. Skipping message.");
            *skipped_count = 1;
            None
        }
        Some(item_json) => match serde_json::from_str::<CreateItemCommandData>(&item_json) {
            Ok(command) => {
                message_ids.insert(command.item_key(), message_id);
                Some(command)
            }
            Err(e) => {
                error!(
                    error = %e,
                    payload = %item_json,
                    "Failed deserializing CreateItemCommandData."
                );
                failed_message_ids.push(message_id);
                None
            }
        },
    }
}

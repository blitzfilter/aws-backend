use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::has_key::HasKey;
use common::item_id::ItemKey;
use item_core::item::command::UpdateItemCommand;
use item_core::item::command_data::UpdateItemCommandData;
use item_service::command_service::CommandItemService;
use lambda_runtime::LambdaEvent;
use std::collections::HashMap;
use tracing::{error, info, warn};

#[tracing::instrument(skip(service, event), fields(requestId = %event.context.request_id))]
pub async fn handler(
    service: &impl CommandItemService,
    event: LambdaEvent<SqsEvent>,
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let records_count = event.payload.records.len();
    info!(total = records_count, "Handler invoked.",);

    let mut failed_message_ids = Vec::new();
    let mut skipped_count = 0;
    let mut commands = HashMap::with_capacity(records_count);
    let mut message_ids: HashMap<ItemKey, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some((item_key, command)) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            commands.insert(item_key, command);
        }
    }

    let failed_command_keys = service
        .handle_update_items(commands)
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
) -> Option<(ItemKey, UpdateItemCommand)> {
    let message_id = message
        .message_id
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");

    match message.body {
        None => {
            info!("Received empty body. Skipping message.");
            *skipped_count += 1;
            None
        }
        Some(item_json) => match serde_json::from_str::<UpdateItemCommandData>(&item_json) {
            Ok(command_data) => {
                let item_key = command_data.key();
                let command = UpdateItemCommand::from(command_data);
                message_ids.insert(item_key.clone(), message_id);
                Some((item_key, command))
            }
            Err(e) => {
                error!(
                    error = %e,
                    payload = %item_json,
                    type = %std::any::type_name::<UpdateItemCommandData>(),
                    "Failed deserializing UpdateItemCommandData."
                );
                failed_message_ids.push(message_id);
                None
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::handler;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::item_id::ItemKey;
    use common::shop_id::ShopId;
    use item_core::item::command_data::UpdateItemCommandData;
    use item_core::item_state::command_data::ItemStateCommandData;
    use item_service::command_service::MockCommandItemService;
    use lambda_runtime::{Context, LambdaEvent};

    #[rstest::rstest]
    #[case::one(1)]
    #[case::five(5)]
    #[case::ten(10)]
    #[case::fifty(50)]
    #[case::fivehundred(500)]
    #[case::onethousand(1000)]
    #[case::tenthousand(10000)]
    #[tokio::test]
    async fn should_pass_on_service_failures(#[case] n: usize) {
        let shop_id = ShopId::new();
        let mk_message = |x: usize| {
            let command_data = UpdateItemCommandData {
                shop_id: shop_id.clone(),
                shops_item_id: x.to_string().into(),
                price: None,
                state: Some(ItemStateCommandData::Listed),
            };
            SqsMessage {
                message_id: Some(x.to_string()),
                receipt_handle: None,
                body: Some(serde_json::to_string(&command_data).unwrap()),
                md5_of_body: None,
                md5_of_message_attributes: None,
                attributes: Default::default(),
                message_attributes: Default::default(),
                event_source_arn: None,
                event_source: None,
                aws_region: None,
            }
        };
        let records = (1..=n).map(mk_message).collect();
        let lambda_event = LambdaEvent {
            payload: SqsEvent { records },
            context: Context::default(),
        };

        let mut service_mock = MockCommandItemService::default();
        service_mock
            .expect_handle_update_items()
            .return_once(move |_| {
                Box::pin(async move { Err(vec![ItemKey::new(shop_id, n.to_string().into())]) })
            });
        let response = handler(&service_mock, lambda_event).await.unwrap();

        assert_eq!(response.batch_item_failures.len(), 1);
    }
}

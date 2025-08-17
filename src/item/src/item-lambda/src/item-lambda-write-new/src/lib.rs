use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent, SqsMessage};
use common::has_key::HasKey;
use common::item_id::ItemKey;
use item_service::command_service::CommandItemService;
use item_service::item_command::CreateItemCommand;
use item_service::item_command_data::CreateItemCommandData;
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
    let mut commands = Vec::with_capacity(records_count);
    let mut message_ids: HashMap<ItemKey, String> = HashMap::with_capacity(records_count);

    for message in event.payload.records {
        if let Some(command) = extract_message_data(
            message,
            &mut failed_message_ids,
            &mut skipped_count,
            &mut message_ids,
        ) {
            commands.push(command);
        }
    }

    let failed_command_keys = service
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
) -> Option<CreateItemCommand> {
    let message_id = message
        .message_id
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");

    match message.body {
        None => {
            info!("Received empty body. Skipping message.");
            *skipped_count += 1;
            None
        }
        Some(item_json) => match serde_json::from_str::<CreateItemCommandData>(&item_json) {
            Ok(command_data) => {
                let command = CreateItemCommand::from(command_data);
                message_ids.insert(command.key(), message_id);
                Some(command)
            }
            Err(e) => {
                error!(
                    error = %e,
                    payload = %item_json,
                    type = %std::any::type_name::<CreateItemCommandData>(),
                    "Failed deserializing CreateItemCommandData."
                );
                failed_message_ids.push(message_id);
                None
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::handler;
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
    use common::has_key::HasKey;
    use fake::{Fake, Faker};
    use item_service::command_service::MockCommandItemService;
    use item_service::item_command_data::CreateItemCommandData;
    use lambda_runtime::{Context, LambdaEvent};
    use uuid::Uuid;

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
    #[tokio::test]
    async fn should_handle_message(#[case] record_count: usize) {
        let records = fake::vec![CreateItemCommandData; record_count]
            .into_iter()
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

        let mut service_mock = MockCommandItemService::default();
        service_mock
            .expect_handle_create_items()
            .return_once(|_| Box::pin(async { Ok(()) }));
        let response = handler(&service_mock, lambda_event).await.unwrap();

        assert!(response.batch_item_failures.is_empty());
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
    async fn should_respond_with_partial_failures(
        #[case] failure_count: usize,
        #[case] record_count: usize,
    ) {
        let commands = fake::vec![CreateItemCommandData; record_count];
        let expected_failed_keys = commands
            .iter()
            .take(failure_count)
            .map(CreateItemCommandData::key)
            .collect::<Vec<_>>();
        let mut messages_ids = HashMap::with_capacity(record_count);
        let records = commands
            .into_iter()
            .map(|cmd_data| {
                let message_id = Uuid::new_v4().to_string();
                messages_ids.insert(cmd_data.key(), message_id.clone());
                SqsMessage {
                    message_id: Some(message_id),
                    receipt_handle: None,
                    body: Some(serde_json::to_string(&cmd_data).unwrap()),
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
        let mut expected_failed_message_ids = expected_failed_keys
            .iter()
            .map(|key| messages_ids.remove(key).unwrap())
            .collect::<Vec<_>>();
        expected_failed_message_ids.sort();
        let lambda_event = LambdaEvent {
            payload: SqsEvent { records },
            context: Context::default(),
        };

        let mut service_mock = MockCommandItemService::default();
        service_mock
            .expect_handle_create_items()
            .return_once(move |_| Box::pin(async { Err(expected_failed_keys) }));
        let mut actual_failed_message_ids = handler(&service_mock, lambda_event)
            .await
            .unwrap()
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>();
        actual_failed_message_ids.sort();

        assert_eq!(failure_count, actual_failed_message_ids.len());
        assert_eq!(expected_failed_message_ids, actual_failed_message_ids);
    }
}

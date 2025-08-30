use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::SqsMessage;
use item_dynamodb::item_event_record::ItemEventRecord;
use tracing::{error, info};

#[tracing::instrument(
    skip(message, failed_message_ids, skipped_count),
    fields(messageId = %message.message_id.as_ref().expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it."))
)]
pub fn extract_item_event_record(
    message: SqsMessage,
    failed_message_ids: &mut Vec<String>,
    skipped_count: &mut usize,
) -> Option<ItemEventRecord> {
    let message_id = message
        .message_id
        .expect("shouldn't receive an SQS-Message without 'message_id' because AWS sets it.");

    match message.body {
        None => {
            info!("Received empty body. Skipping message.");
            *skipped_count += 1;
            None
        }
        Some(event_bridge_event_json) => {
            match serde_json::from_str::<EventBridgeEvent<aws_lambda_events::dynamodb::EventRecord>>(
                &event_bridge_event_json,
            ) {
                Ok(event_bridge_event) => match serde_dynamo::from_item::<_, ItemEventRecord>(
                    event_bridge_event.detail.change.new_image,
                ) {
                    Ok(item_event_record) => Some(item_event_record),
                    Err(e) => {
                        error!(
                            error = %e,
                            type = %std::any::type_name::<ItemEventRecord>(),
                            payload = %event_bridge_event_json,
                            "Failed deserializing 'detail.new_image'."
                        );
                        failed_message_ids.push(message_id);
                        None
                    }
                },
                Err(e) => {
                    error!(
                        error = %e,
                        type = %std::any::type_name::<EventBridgeEvent<aws_lambda_events::dynamodb::EventRecord>>(),
                        payload = %event_bridge_event_json,
                        "Failed deserializing."
                    );
                    failed_message_ids.push(message_id);
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::extract_item_event_record;
    use aws_lambda_events::{
        dynamodb::{EventRecord, StreamRecord},
        eventbridge::EventBridgeEvent,
        sqs::SqsMessage,
    };
    use fake::{Fake, Faker};
    use item_dynamodb::item_event_record::ItemEventRecord;
    use std::time::SystemTime;
    use uuid::Uuid;

    #[test]
    fn should_fail_when_invalid_json() {
        let msg = SqsMessage {
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

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_item_event_record(msg, &mut failed_message_ids, &mut skipped_count);

        assert!(actual.is_none());
        assert_eq!(vec!["msg1".to_string()], failed_message_ids);
        assert_eq!(0, skipped_count);
    }

    #[test]
    fn should_skip_message_for_empty_message_body() {
        let msg = SqsMessage {
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

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_item_event_record(msg, &mut failed_message_ids, &mut skipped_count);

        assert!(actual.is_none());
        assert!(failed_message_ids.is_empty());
        assert_eq!(1, skipped_count);
    }

    #[test]
    fn should_fail_when_valid_json_cannot_be_deserialized_to_target_type() {
        let invalid_conversion_json = r#"{"eventType":"Created","shopId":"test","shopsItemId":"test","timestamp":"2023-01-01T00:00:00Z","boop":{"item":null}}"#;
        let msg = SqsMessage {
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

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_item_event_record(msg, &mut failed_message_ids, &mut skipped_count);

        assert!(actual.is_none());
        assert_eq!(vec!["test_msg".to_string()], failed_message_ids);
        assert_eq!(0, skipped_count);
    }

    #[test]
    fn should_succeed_extract_message_data_with_valid_data() {
        let expected = Faker.fake::<ItemEventRecord>();
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
                    new_image: serde_dynamo::to_item(&expected).unwrap(),
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
        let msg = SqsMessage {
            message_id: Some("test_msg".to_string()),
            receipt_handle: None,
            body: Some(serde_json::to_string(&event).unwrap()),
            md5_of_body: None,
            md5_of_message_attributes: None,
            attributes: Default::default(),
            message_attributes: Default::default(),
            event_source_arn: None,
            event_source: None,
            aws_region: None,
        };

        let mut failed_message_ids = vec![];
        let mut skipped_count = 0;
        let actual = extract_item_event_record(msg, &mut failed_message_ids, &mut skipped_count);

        assert!(actual.is_some());
        assert_eq!(expected, actual.unwrap());
        assert!(failed_message_ids.is_empty());
        assert_eq!(0, skipped_count);
    }
}

use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use common::price::domain::FixedFxRate;
use item_core::item::command_data::UpdateItemCommandData;
use item_core::item_state::command_data::ItemStateCommandData;
use item_lambda_write_update::handler;
use item_write::service::CommandItemServiceContext;
use lambda_runtime::{Context, LambdaEvent};
use test_api::*;

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::fifty(50)]
#[case::fivehundred(500)]
#[case::onethousand(1000)]
#[localstack_test(services = [DynamoDB()])]
async fn should_skip_all_when_they_dont_exist(#[case] n: usize) {
    let mk_message = |x: usize| {
        let command_data = UpdateItemCommandData {
            shop_id: Default::default(),
            shops_item_id: Default::default(),
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

    let client = get_dynamodb_client().await;
    let service_context = &CommandItemServiceContext {
        dynamodb_client: client,
        fx_rate: &FixedFxRate::default(),
    };
    let response = handler(service_context, lambda_event).await.unwrap();

    assert!(response.batch_item_failures.is_empty());

    let scan_result = client
        .scan()
        .table_name(get_dynamodb_table_name())
        .send()
        .await
        .unwrap();
    assert_eq!(0, scan_result.count as usize);
}

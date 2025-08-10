use std::vec;

use aws_lambda_events::sqs::SqsEvent;
use aws_lambda_events::sqs::SqsMessage;
use common::env::get_dynamodb_table_name;
use common::localized::Localized;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::domain::Item;
use item_core::item::domain::shop_name::ShopName;
use item_core::item_event::record::ItemEventRecord;
use item_lambda_materialize_dynamodb_new::handler;
use lambda_runtime::{Context, LambdaEvent};
use test_api::*;
use url::Url;

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::fortytwo(42)]
#[case::fivehundred(500)]
#[case::onethousand(1000)]
#[localstack_test(services = [DynamoDB()])]
async fn should_materialize_items_for_create(#[case] n: usize) {
    let mk_message = |x: usize| {
        let event_record: ItemEventRecord = Item::create(
            ShopId::new(),
            ShopsItemId::new(),
            ShopName::from("boop"),
            Localized::new(
                common::language::domain::Language::De,
                "Title boooop".into(),
            ),
            Default::default(),
            None,
            Default::default(),
            Some(Price::new(
                50000u64.into(),
                common::currency::domain::Currency::Cad,
            )),
            Default::default(),
            item_core::item_state::domain::ItemState::Available,
            Url::parse("https://boop.bap.com").unwrap(),
            vec![],
        )
        .try_into()
        .unwrap();
        SqsMessage {
            message_id: Some(x.to_string()),
            receipt_handle: None,
            body: Some(serde_json::to_string(&event_record).unwrap()),
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
    let response = handler(client, lambda_event).await.unwrap();

    assert!(response.batch_item_failures.is_empty());

    let scan_result = client
        .scan()
        .table_name(get_dynamodb_table_name())
        .send()
        .await
        .unwrap();
    assert_eq!(n, scan_result.count as usize);
}

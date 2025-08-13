use aws_lambda_events::sqs::SqsEvent;
use aws_lambda_events::sqs::SqsMessage;
use common::item_id::ItemId;
use common::localized::Localized;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::document::ItemDocument;
use item_core::item::domain::Item;
use item_core::item::domain::shop_name::ShopName;
use item_core::item_event::record::ItemEventRecord;
use item_lambda_materialize_opensearch_new::handler;
use item_opensearch::ItemOpenSearchRepositoryImpl;
use lambda_runtime::{Context, LambdaEvent};
use std::vec;
use test_api::*;
use url::Url;

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::fortytwo(42)]
#[case::fivehundred(500)]
#[localstack_test(services = [OpenSearch()])]
async fn should_materialize_items_for_create(#[case] n: usize) {
    let mut item_ids: Vec<ItemId> = Vec::with_capacity(n);
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
        item_ids.push(event_record.item_id);
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

    let client = get_opensearch_client().await;
    let repository = ItemOpenSearchRepositoryImpl::new(client);
    let response = handler(&repository, lambda_event).await.unwrap();
    assert!(response.batch_item_failures.is_empty());

    refresh_index("items").await;
    for item_id in item_ids {
        let actual = read_by_id::<ItemDocument>("items", item_id).await;
        assert!(actual.is_available);
    }
}

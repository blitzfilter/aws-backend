use common::batch::Batch;
use common::item_id::ItemKey;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::command::{CreateItemCommand, UpdateItemCommand};
use item_core::item::hash::ItemHash;
use item_core::item::record::ItemRecord;
use item_core::item_event::domain::{ItemEvent, ItemEventPayload};
use item_core::item_event::record::ItemEventRecord;
use item_core::item_state::domain::ItemState;
use item_core::item_state::record::ItemStateRecord;
use item_write::repository::WriteItemRecords;
use item_write::service::InboundWriteItems;
use test_api::*;
use time::OffsetDateTime;

#[localstack_test(services = [DynamoDB()])]
async fn should_create_items_for_handle_create_items_with_one_command() {
    let shop_id = ShopId::new();
    let shops_item_id = ShopsItemId::new();
    let cmd = CreateItemCommand {
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Boop".to_string(),
        title: Default::default(),
        description: Default::default(),
        price: None,
        state: ItemState::Listed,
        url: "https://beep.boop.com/baap".to_string(),
        images: vec![],
    };

    let client = get_dynamodb_client().await;
    let write_res = client.handle_create_items(vec![cmd.clone()]).await;
    assert!(write_res.is_ok());

    let event_record_attr_map = client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()[0]
        .clone();
    let event_record =
        serde_dynamo::from_item::<_, ItemEventRecord>(event_record_attr_map).unwrap();
    let event: ItemEvent = event_record.try_into().unwrap();
    match event.payload {
        ItemEventPayload::Created(payload) => {
            assert_eq!(cmd.shop_id, payload.shop_id);
            assert_eq!(cmd.shops_item_id, payload.shops_item_id);
            assert_eq!(cmd.shop_name, payload.shop_name);
            assert_eq!(cmd.title, payload.title);
            assert_eq!(cmd.description, payload.description);
            assert_eq!(cmd.price, payload.price);
            assert_eq!(cmd.state, payload.state);
            assert_eq!(cmd.url, payload.url);
            assert_eq!(cmd.images, payload.images);
        }
        _ => panic!("Expected ItemEventPayload::Created."),
    }
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::ten(10)]
#[case::twentyfive(25)]
#[case::fortytwo(42)]
#[case::fifty(50)]
#[case::sixtynine(69)]
#[case::onehundred(100)]
#[case::fourhundredandtwenty(420)]
#[case::fivehundred(500)]
#[localstack_test(services = [DynamoDB()])]
async fn should_create_items_for_handle_create_items_with_commands_count(#[case] count: i32) {
    let shop_id = ShopId::new();
    let mk_cmd = |x: i32| CreateItemCommand {
        shop_id: shop_id.clone(),
        shops_item_id: ShopsItemId::from(x.to_string()),
        shop_name: "Boop".to_string(),
        title: Default::default(),
        description: Default::default(),
        price: None,
        state: ItemState::Listed,
        url: "https://beep.boop.com/baap".to_string(),
        images: vec![],
    };
    let cmds = (1..=count).map(mk_cmd).collect();
    let client = get_dynamodb_client().await;
    let write_res = client.handle_create_items(cmds).await;
    assert!(write_res.is_ok());

    let actual_count = client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .count;

    assert_eq!(count, actual_count);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_partially_skip_existent_items_for_handle_create_items() {
    let shop_id = ShopId::new();
    let shops_item_id = ShopsItemId::new();
    let cmd = CreateItemCommand {
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "Boop".to_string(),
        title: Default::default(),
        description: Default::default(),
        price: None,
        state: ItemState::Reserved,
        url: "https://beep.boop.com/baap".to_string(),
        images: vec![],
    };

    let client = get_dynamodb_client().await;
    let write_res_1 = client.handle_create_items(vec![cmd.clone()]).await;
    assert!(write_res_1.is_ok());

    // manually insert the materialized one
    let materialized = ItemRecord {
        pk: format!(
            "item#shop_id#{}#shops_item_id#{}",
            shop_id.clone(),
            shops_item_id.clone()
        ),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{}", shop_id.clone()),
        gsi_1_sk: "updated#2007-12-24T18:21Z".to_string(),
        item_id: Default::default(),
        event_id: Default::default(),
        shop_id: shop_id.clone(),
        shops_item_id: shops_item_id.clone(),
        shop_name: "".to_string(),
        title: None,
        title_de: None,
        title_en: None,
        description: None,
        description_de: None,
        description_en: None,
        price: None,
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Reserved,
        url: "".to_string(),
        images: vec![],
        hash: ItemHash::new(&None, &ItemState::Reserved),
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let write_materialized_output = client
        .put_item_records(Batch::from([materialized]))
        .await
        .unwrap();
    assert!(
        write_materialized_output
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );
    let actual_count_1 = client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .count;
    assert_eq!(2, actual_count_1);

    // Attempting to write created-event again is successful, but skipped
    let write_res_2 = client.handle_create_items(vec![cmd.clone()]).await;
    assert!(write_res_2.is_ok());
    let actual_count_2 = client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .count;
    assert_eq!(actual_count_1, actual_count_2);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::ten(10)]
#[case::twentyfive(25)]
#[case::fortytwo(42)]
#[case::fifty(50)]
#[case::sixtynine(69)]
#[case::onehundred(100)]
#[case::fourhundredandtwenty(420)]
#[case::fivehundred(500)]
#[localstack_test(services = [DynamoDB()])]
async fn should_skip_non_existent_items_for_handle_update_items_with_commands_count(
    #[case] count: i32,
) {
    let shop_id = ShopId::new();
    let mk_entry = |x: i32| {
        (
            ItemKey::new(shop_id.clone(), ShopsItemId::from(x.to_string())),
            UpdateItemCommand {
                price: None,
                state: Some(ItemState::Listed),
            },
        )
    };
    let cmds = (1..=count).map(mk_entry).collect();
    let client = get_dynamodb_client().await;
    let write_res = client.handle_update_items(cmds).await;
    assert!(write_res.is_ok());

    let actual_count = client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .count;
    assert_eq!(0, actual_count);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::ten(10)]
#[case::twentyfive(25)]
#[case::fortytwo(42)]
#[case::fifty(50)]
#[case::sixtynine(69)]
#[case::onehundred(100)]
#[case::fourhundredandtwenty(420)]
#[case::fivehundred(500)]
#[localstack_test(services = [DynamoDB()])]
async fn should_partially_skip_non_existent_items_for_handle_update_items_with_commands_count(
    #[case] count: i32,
) {
    let shop_id = ShopId::new();
    let cmd = CreateItemCommand {
        shop_id: shop_id.clone(),
        shops_item_id: ShopsItemId::from(count.to_string()),
        shop_name: "Boop".to_string(),
        title: Default::default(),
        description: Default::default(),
        price: None,
        state: ItemState::Reserved,
        url: "https://beep.boop.com/baap".to_string(),
        images: vec![],
    };

    // create only the last item
    let client = get_dynamodb_client().await;
    let write_res = client.handle_create_items(vec![cmd.clone()]).await;
    assert!(write_res.is_ok());

    // manually insert the materialized one
    let materialized = ItemRecord {
        pk: format!("item#shop_id#{}#shops_item_id#{count}", shop_id.clone()),
        sk: "item#materialized".to_string(),
        gsi_1_pk: format!("shop_id#{}", shop_id.clone()),
        gsi_1_sk: "updated#2007-12-24T18:21Z".to_string(),
        item_id: Default::default(),
        event_id: Default::default(),
        shop_id: shop_id.clone(),
        shops_item_id: ShopsItemId::from(count.to_string()),
        shop_name: "".to_string(),
        title: None,
        title_de: None,
        title_en: None,
        description: None,
        description_de: None,
        description_en: None,
        price: None,
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Reserved,
        url: "".to_string(),
        images: vec![],
        hash: ItemHash::new(&None, &ItemState::Reserved),
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };
    let write_materialized_output = client
        .put_item_records(Batch::from([materialized]))
        .await
        .unwrap();
    assert!(
        write_materialized_output
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    // update all items
    let mk_entry = |x: i32| {
        (
            ItemKey::new(shop_id.clone(), ShopsItemId::from(x.to_string())),
            UpdateItemCommand {
                price: None,
                state: Some(ItemState::Listed),
            },
        )
    };
    let cmds = (1..=count).map(mk_entry).collect();
    let client = get_dynamodb_client().await;
    let write_res = client.handle_update_items(cmds).await;
    assert!(write_res.is_ok());

    let actual_count = client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .count;
    assert_eq!(3, actual_count);
}

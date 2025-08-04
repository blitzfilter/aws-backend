use common::batch::Batch;
use common::language::data::LocalizedTextData;
use common::language::record::{LanguageRecord, TextRecord};
use common::{
    currency::data::CurrencyData, event_id::EventId, item_id::ItemId, language::data::LanguageData,
    price::data::PriceData, shop_id::ShopId,
};
use item_core::{
    item::{hash::ItemHash, record::ItemRecord},
    item_state::{data::ItemStateData, domain::ItemState, record::ItemStateRecord},
};
use item_write::repository::PersistItemRepository;
use scrape_core::{
    data::ScrapeItem,
    service::{PublishScrapeItemService, PublishScrapeItemsContext},
};
use std::{collections::HashMap, time::Duration};
use test_api::*;
use time::macros::datetime;
use url::Url;

const CREATE_ITEM_SQS: Sqs = Sqs {
    name: "item-lambda-write-new-queue",
};
const CREATE_ITEM_LAMBDA: Lambda = Lambda {
    name: "item-lambda-write-new",
    path: concat!(
        env!("CARGO_WORKSPACE_DIR"),
        "src/item/src/item-lambda/src/item-lambda-write-new"
    ),
    role: None,
};
const CREATE_ITEM_SQS_LAMBDA: SqsLambdaEventSourceMapping = SqsLambdaEventSourceMapping {
    sqs: &CREATE_ITEM_SQS,
    lambda: &CREATE_ITEM_LAMBDA,
    max_batch_size: 1000,
    max_batch_window_seconds: 3,
};

const UPDATE_ITEM_SQS: Sqs = Sqs {
    name: "item-lambda-write-update-queue",
};
const UPDATE_ITEM_LAMBDA: Lambda = Lambda {
    name: "item-lambda-write-update",
    path: concat!(
        env!("CARGO_WORKSPACE_DIR"),
        "src/item/src/item-lambda/src/item-lambda-write-update"
    ),
    role: None,
};
const UPDATE_ITEM_SQS_LAMBDA: SqsLambdaEventSourceMapping = SqsLambdaEventSourceMapping {
    sqs: &UPDATE_ITEM_SQS,
    lambda: &UPDATE_ITEM_LAMBDA,
    max_batch_size: 1000,
    max_batch_window_seconds: 3,
};

fn mk_scrape_item(id: usize, shop_id: &ShopId) -> ScrapeItem {
    ScrapeItem {
        shop_id: shop_id.clone(),
        shops_item_id: id.to_string().into(),
        shop_name: "Lorem ipsum".to_owned(),
        native_title: LocalizedTextData {
            text: "boop".to_string(),
            language: LanguageData::De,
        },
        other_title: HashMap::from([
            (LanguageData::De, "Deutscher Titel".to_owned()),
            (LanguageData::En, "English title".to_owned()),
            (LanguageData::Fr, "Français titre".to_owned()),
            (LanguageData::Es, "Español título".to_owned()),
        ]),
        native_description: None,
        other_description: HashMap::from([
            (LanguageData::De, "Deutsche Beschreibung".to_owned()),
            (LanguageData::En, "English description".to_owned()),
            (LanguageData::Fr, "Français description".to_owned()),
            (LanguageData::Es, "Español descripción".to_owned()),
        ]),
        price: Some(PriceData {
            currency: CurrencyData::Eur,
            amount: 10000u32.into(),
        }),
        state: ItemStateData::Available,
        url: "https://example.com/Lorem".to_owned(),
        images: vec![
            "https://example.com/Lorem/image1.jpg".to_owned(),
            "https://example.com/Lorem/image2.jpg".to_owned(),
            "https://example.com/Lorem/image3.jpg".to_owned(),
        ],
    }
}

fn mk_item_record(id: usize, shop_id: &ShopId) -> ItemRecord {
    ItemRecord {
        pk: format!("item#shop_id#{shop_id}#shops_item_id#{id}"),
        sk: "item#materialized".into(),
        gsi_1_pk: format!("shop_id#{shop_id}"),
        gsi_1_sk: "updated#2007-12-24T18:21Z".to_string(),
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: shop_id.clone(),
        shops_item_id: id.to_string().into(),
        shop_name: "Lorem ipsum".into(),
        title_native: TextRecord::new("Boopsie whoop", LanguageRecord::En),
        title_de: None,
        title_en: None,
        description_native: None,
        description_de: None,
        description_en: None,
        price_native: None,
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        state: ItemStateRecord::Listed,
        url: Url::parse(&format!("https://example.com/{id}")).unwrap(),
        images: vec![],
        hash: ItemHash::new(&None, &ItemState::Listed),
        created: datetime!(2007 - 12 - 24 18:21 UTC),
        updated: datetime!(2007 - 12 - 24 18:21 UTC),
    }
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(100)]
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS_LAMBDA])]
async fn should_publish_scrape_items_then_find_them_in_dynamodb_for_create(#[case] n: usize) {
    let service = PublishScrapeItemsContext {
        dynamodb_client: get_dynamodb_client().await,
        sqs_client: get_sqs_client().await,
        sqs_create_url: CREATE_ITEM_SQS.queue_url(),
        sqs_update_url: UPDATE_ITEM_SQS.queue_url(),
    };
    let shop_ids = HashMap::from([
        (0, ShopId::new()),
        (1, ShopId::new()),
        (2, ShopId::new()),
        (3, ShopId::new()),
        (4, ShopId::new()),
        (5, ShopId::new()),
        (6, ShopId::new()),
        (7, ShopId::new()),
        (8, ShopId::new()),
    ]);
    let scrape_items = (1..=n)
        .map(|i| mk_scrape_item(i, shop_ids.get(&(i % 9)).unwrap()))
        .collect::<Vec<_>>();

    let publish_res = service.publish_scrape_items(scrape_items).await;
    assert!(publish_res.is_ok());

    // Wait for Lambda to consume and handle all messages
    tokio::time::sleep(Duration::from_secs(10)).await;

    let actual = service
        .dynamodb_client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .count;

    assert_eq!(n, actual as usize);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(50)]
#[localstack_test(services = [DynamoDB(), UPDATE_ITEM_SQS_LAMBDA])]
async fn should_publish_scrape_items_then_find_them_in_dynamodb_for_update(#[case] n: usize) {
    let service = PublishScrapeItemsContext {
        dynamodb_client: get_dynamodb_client().await,
        sqs_client: get_sqs_client().await,
        sqs_create_url: CREATE_ITEM_SQS.queue_url(),
        sqs_update_url: UPDATE_ITEM_SQS.queue_url(),
    };

    // Simulate materialized view
    let shop_ids = HashMap::from([
        (0, ShopId::new()),
        (1, ShopId::new()),
        (2, ShopId::new()),
        (3, ShopId::new()),
        (4, ShopId::new()),
        (5, ShopId::new()),
        (6, ShopId::new()),
        (7, ShopId::new()),
        (8, ShopId::new()),
    ]);
    for batch in Batch::<_, 25>::chunked_from(
        (1..=n).map(|i| mk_item_record(i, shop_ids.get(&(i % 9)).unwrap())),
    ) {
        service
            .dynamodb_client
            .put_item_records(batch)
            .await
            .unwrap();
    }

    // Publish updates
    let scrape_items = (1..=n)
        .map(|i| mk_scrape_item(i, shop_ids.get(&(i % 9)).unwrap()))
        .collect::<Vec<_>>();

    let publish_res = service.publish_scrape_items(scrape_items).await;
    assert!(publish_res.is_ok());

    // Wait for Lambda to consume and handle all messages
    tokio::time::sleep(Duration::from_secs(10)).await;

    let actual = service
        .dynamodb_client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .count;

    // one materialized and one update state and one update price
    assert_eq!(3 * n, actual as usize);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(50)]
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS_LAMBDA, UPDATE_ITEM_SQS_LAMBDA])]
async fn should_publish_scrape_items_then_find_them_in_dynamodb_for_create_update_mix(
    #[case] n: usize,
) {
    let service = PublishScrapeItemsContext {
        dynamodb_client: get_dynamodb_client().await,
        sqs_client: get_sqs_client().await,
        sqs_create_url: CREATE_ITEM_SQS.queue_url(),
        sqs_update_url: UPDATE_ITEM_SQS.queue_url(),
    };

    // Simulate materialized view
    let shop_ids = HashMap::from([
        (0, ShopId::new()),
        (1, ShopId::new()),
        (2, ShopId::new()),
        (3, ShopId::new()),
        (4, ShopId::new()),
        (5, ShopId::new()),
        (6, ShopId::new()),
        (7, ShopId::new()),
        (8, ShopId::new()),
    ]);
    let materialized_items = (1..=n)
        .filter(|i| i % 3 == 0)
        .map(|i| mk_item_record(i, shop_ids.get(&(i % 9)).unwrap()));
    let expected_update_items = materialized_items.clone().count();
    let expected_new_items = n - expected_update_items;
    for batch in Batch::<_, 25>::chunked_from(materialized_items) {
        service
            .dynamodb_client
            .put_item_records(batch)
            .await
            .unwrap();
    }

    // Publish updates
    let scrape_items = (1..=n)
        .map(|i| mk_scrape_item(i, shop_ids.get(&(i % 9)).unwrap()))
        .collect::<Vec<_>>();

    let publish_res = service.publish_scrape_items(scrape_items).await;
    assert!(publish_res.is_ok());

    // Wait for Lambda to consume and handle all messages
    tokio::time::sleep(Duration::from_secs(10)).await;

    let actual = service
        .dynamodb_client
        .scan()
        .table_name("items")
        .send()
        .await
        .unwrap()
        .count;

    // every third is an update
    // for update: one materialized and one update state and one update price
    assert_eq!(
        expected_new_items + 3 * expected_update_items,
        actual as usize
    );
}

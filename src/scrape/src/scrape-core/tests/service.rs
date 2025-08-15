use common::language::data::LocalizedTextData;
use common::language::record::{LanguageRecord, TextRecord};
use common::{
    batch::Batch, currency::data::CurrencyData, event_id::EventId, item_id::ItemId,
    language::data::LanguageData, price::data::PriceData, shop_id::ShopId,
};
use item_core::hash::ItemHash;
use item_core::item_state::ItemState;
use item_data::item_state_data::ItemStateData;
use item_dynamodb::item_record::ItemRecord;
use item_dynamodb::item_state_record::ItemStateRecord;
use item_dynamodb::repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl};
use scrape_core::{
    data::ScrapeItem,
    service::{PublishScrapeItemService, PublishScrapeItemsImpl},
};
use std::collections::HashMap;
use test_api::*;
use time::macros::datetime;
use url::Url;

const CREATE_ITEM_SQS: Sqs = Sqs {
    name: "item-lambda-write-new-queue",
};
const UPDATE_ITEM_SQS: Sqs = Sqs {
    name: "item-lambda-write-update-queue",
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
        url: Url::parse("https://foo.bar").unwrap(),
        images: vec![
            Url::parse("https://example.com/Lorem/image1.jpg").unwrap(),
            Url::parse("https://example.com/Lorem/image2.jpg").unwrap(),
            Url::parse("https://example.com/Lorem/image3.jpg").unwrap(),
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
#[case::onethousand(1000)]
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS])]
async fn should_publish_scrape_items_for_create_for_single_shop_id(#[case] n: usize) {
    let service = PublishScrapeItemsImpl {
        dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await),
        sqs_client: get_sqs_client().await,
        sqs_create_url: CREATE_ITEM_SQS.queue_url(),
        sqs_update_url: UPDATE_ITEM_SQS.queue_url(),
    };
    let shop_id = ShopId::new();
    let scrape_items = (1..=n)
        .map(|i| mk_scrape_item(i, &shop_id))
        .collect::<Vec<_>>();

    let publish_res = service.publish_scrape_items(scrape_items).await;
    assert!(publish_res.is_ok());

    let mut actual_count: usize = 0;
    loop {
        let received = service
            .sqs_client
            .receive_message()
            .queue_url(CREATE_ITEM_SQS.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap();
        if received.messages().is_empty() {
            break;
        } else {
            actual_count += received.messages().len();
        }
    }

    assert_eq!(n, actual_count)
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(100)]
#[case::onethousand(1000)]
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS])]
async fn should_publish_scrape_items_for_create_for_multiple_shop_ids(#[case] n: usize) {
    let service = PublishScrapeItemsImpl {
        dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await),
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

    let mut actual_count: usize = 0;
    loop {
        let received = service
            .sqs_client
            .receive_message()
            .queue_url(CREATE_ITEM_SQS.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap();
        if received.messages().is_empty() {
            break;
        } else {
            actual_count += received.messages().len();
        }
    }

    assert_eq!(n, actual_count)
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(100)]
#[case::onethousand(1000)]
#[localstack_test(services = [DynamoDB(), UPDATE_ITEM_SQS])]
async fn should_publish_scrape_items_for_update_for_single_shop_id(#[case] n: usize) {
    let service = PublishScrapeItemsImpl {
        dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await),
        sqs_client: get_sqs_client().await,
        sqs_create_url: CREATE_ITEM_SQS.queue_url(),
        sqs_update_url: UPDATE_ITEM_SQS.queue_url(),
    };
    let dynamodb_write_repository = &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await);

    // Simulate materialized view
    let shop_id = ShopId::new();
    for batch in Batch::<_, 25>::chunked_from((1..=n).map(|i| mk_item_record(i, &shop_id))) {
        dynamodb_write_repository
            .put_item_records(batch)
            .await
            .unwrap();
    }

    // Publish updates
    let scrape_items = (1..=n)
        .map(|i| mk_scrape_item(i, &shop_id))
        .collect::<Vec<_>>();

    let publish_res = service.publish_scrape_items(scrape_items).await;
    assert!(publish_res.is_ok());

    // Verify Queue-Content
    let mut actual_count: usize = 0;
    loop {
        let received = service
            .sqs_client
            .receive_message()
            .queue_url(UPDATE_ITEM_SQS.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap();
        if received.messages().is_empty() {
            break;
        } else {
            actual_count += received.messages().len();
        }
    }

    assert_eq!(n, actual_count)
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(100)]
#[case::onethousand(1000)]
#[localstack_test(services = [DynamoDB(), UPDATE_ITEM_SQS])]
async fn should_publish_scrape_items_for_update_for_multiple_shop_ids(#[case] n: usize) {
    let service = PublishScrapeItemsImpl {
        dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await),
        sqs_client: get_sqs_client().await,
        sqs_create_url: CREATE_ITEM_SQS.queue_url(),
        sqs_update_url: UPDATE_ITEM_SQS.queue_url(),
    };
    let dynamodb_write_repository = &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await);

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
        dynamodb_write_repository
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

    // Verify Queue-Content
    let mut actual_count: usize = 0;
    loop {
        let received = service
            .sqs_client
            .receive_message()
            .queue_url(UPDATE_ITEM_SQS.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap();
        if received.messages().is_empty() {
            break;
        } else {
            actual_count += received.messages().len();
        }
    }

    assert_eq!(n, actual_count)
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(100)]
#[case::onethousand(1000)]
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS, UPDATE_ITEM_SQS])]
async fn should_publish_scrape_items_for_downstream_command_mix_for_single_shop_id(
    #[case] n: usize,
) {
    let service = PublishScrapeItemsImpl {
        dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await),
        sqs_client: get_sqs_client().await,
        sqs_create_url: CREATE_ITEM_SQS.queue_url(),
        sqs_update_url: UPDATE_ITEM_SQS.queue_url(),
    };
    let dynamodb_write_repository = &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await);

    // Simulate materialized view
    let shop_id = ShopId::new();
    for batch in Batch::<_, 25>::chunked_from(
        (1..=n)
            .filter(|i| i % 3 == 0)
            .map(|i| mk_item_record(i, &shop_id)),
    ) {
        dynamodb_write_repository
            .put_item_records(batch)
            .await
            .unwrap();
    }

    // Publish updates
    let scrape_items = (1..=n)
        .map(|i| mk_scrape_item(i, &shop_id))
        .collect::<Vec<_>>();

    let publish_res = service.publish_scrape_items(scrape_items).await;
    assert!(publish_res.is_ok());

    // Verify Queue-Contents
    let mut actual_count_create: usize = 0;
    loop {
        let received = service
            .sqs_client
            .receive_message()
            .queue_url(CREATE_ITEM_SQS.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap();
        if received.messages().is_empty() {
            break;
        } else {
            actual_count_create += received.messages().len();
        }
    }

    let mut actual_count_update: usize = 0;
    loop {
        let received = service
            .sqs_client
            .receive_message()
            .queue_url(UPDATE_ITEM_SQS.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap();
        if received.messages().is_empty() {
            break;
        } else {
            actual_count_update += received.messages().len();
        }
    }

    assert_eq!(n / 3, actual_count_update);
    assert_eq!(n, actual_count_create + actual_count_update);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(100)]
#[case::onethousand(1000)]
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS, UPDATE_ITEM_SQS])]
async fn should_publish_scrape_items_for_downstream_command_mix_for_multiple_shop_ids(
    #[case] n: usize,
) {
    let service = PublishScrapeItemsImpl {
        dynamodb_repository: &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await),
        sqs_client: get_sqs_client().await,
        sqs_create_url: CREATE_ITEM_SQS.queue_url(),
        sqs_update_url: UPDATE_ITEM_SQS.queue_url(),
    };
    let dynamodb_write_repository = &ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await);

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
        (1..=n)
            .filter(|i| i % 3 == 0)
            .map(|i| mk_item_record(i, shop_ids.get(&(i % 9)).unwrap())),
    ) {
        dynamodb_write_repository
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

    // Verify Queue-Contents
    let mut actual_count_create: usize = 0;
    loop {
        let received = service
            .sqs_client
            .receive_message()
            .queue_url(CREATE_ITEM_SQS.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap();
        if received.messages().is_empty() {
            break;
        } else {
            actual_count_create += received.messages().len();
        }
    }

    let mut actual_count_update: usize = 0;
    loop {
        let received = service
            .sqs_client
            .receive_message()
            .queue_url(UPDATE_ITEM_SQS.queue_url())
            .max_number_of_messages(10)
            .send()
            .await
            .unwrap();
        if received.messages().is_empty() {
            break;
        } else {
            actual_count_update += received.messages().len();
        }
    }

    assert_eq!(n / 3, actual_count_update);
    assert_eq!(n, actual_count_create + actual_count_update);
}

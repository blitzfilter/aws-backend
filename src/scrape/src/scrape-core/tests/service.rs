use common::{
    currency::data::CurrencyData, language::data::LanguageData, price::data::PriceData,
    shop_id::ShopId,
};
use item_core::item_state::data::ItemStateData;
use scrape_core::{
    data::ScrapeItem,
    service::{PublishScrapeItems, PublishScrapeItemsContext},
};
use std::collections::HashMap;
use test_api::*;

const CREATE_ITEM_SQS: Sqs = Sqs {
    name: "item-lambda-write-new-queue",
};
const UPDATE_ITEM_SQS: Sqs = Sqs {
    name: "item-lambda-write-new-queue",
};

fn mk_scrape_item(id: usize, shop_id: &ShopId) -> ScrapeItem {
    ScrapeItem {
        shop_id: shop_id.clone(),
        shops_item_id: id.to_string().into(),
        shop_name: "Lorem ipsum".to_owned(),
        title: HashMap::from([
            (LanguageData::De, "Deutscher Titel".to_owned()),
            (LanguageData::En, "English title".to_owned()),
            (LanguageData::Fr, "Français titre".to_owned()),
            (LanguageData::Es, "Español título".to_owned()),
        ]),
        description: HashMap::from([
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

#[rstest::rstest]
#[test_attr(apply(test))]
#[case::one(1)]
#[case::five(5)]
#[case::ten(10)]
#[case::onehundred(100)]
#[case::onethousand(1000)]
#[case::tenthousand(10000)]
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS])]
async fn should_publish_scrape_items_for_create_for_single_shop_id(#[case] n: usize) {
    let service = PublishScrapeItemsContext {
        dynamodb_client: get_dynamodb_client().await,
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
#[case::tenthousand(10000)]
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS])]
async fn should_publish_scrape_items_for_create_for_multiple_shop_ids(#[case] n: usize) {
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

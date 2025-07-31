use std::collections::HashMap;

use common::{
    currency::command_data::CurrencyCommandData, language::command_data::LanguageCommandData,
    price::command_data::PriceCommandData, shop_id::ShopId,
};
use item_core::{
    item::command_data::CreateItemCommandData, item_state::command_data::ItemStateCommandData,
};
use test_api::*;

const CREATE_ITEM_SQS_LAMBDA: SqsWithLambda = SqsWithLambda {
    name: "item-lambda-write-new-queue",
    lambda: &Lambda {
        name: "item-lambda-write-new",
        path: concat!(
            env!("CARGO_WORKSPACE_DIR"),
            "src/item/src/item-lambda/src/item-lambda-write-new"
        ),
        role: None,
    },
    max_batch_size: 1000,
    max_batch_window_seconds: 3,
};

fn mk_create_item_command_data(id: usize) -> CreateItemCommandData {
    CreateItemCommandData {
        shop_id: ShopId::new(),
        shops_item_id: id.to_string().into(),
        shop_name: "Lorem ipsum".to_owned(),
        title: HashMap::from([
            (LanguageCommandData::De, "Deutscher Titel".to_owned()),
            (LanguageCommandData::En, "English title".to_owned()),
            (LanguageCommandData::Fr, "Français titre".to_owned()),
            (LanguageCommandData::Es, "Español título".to_owned()),
        ]),
        description: HashMap::from([
            (LanguageCommandData::De, "Deutsche Beschreibung".to_owned()),
            (LanguageCommandData::En, "English description".to_owned()),
            (LanguageCommandData::Fr, "Français description".to_owned()),
            (LanguageCommandData::Es, "Español descripción".to_owned()),
        ]),
        price: Some(PriceCommandData {
            currency: CurrencyCommandData::Eur,
            amount: 10000u32.into(),
        }),
        state: ItemStateCommandData::Available,
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
#[localstack_test(services = [DynamoDB(), CREATE_ITEM_SQS_LAMBDA])]
async fn should_publish_scrape_items_for_create_commands(#[case] n: usize) {}

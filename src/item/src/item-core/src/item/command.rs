use crate::item_state::command::ItemStateCommand;
use common::language::command::LanguageCommand;
use common::language::domain::Language;
use common::price::command::PriceCommand;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpsertItemCommand {
    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<HashMap<LanguageCommand, String>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<HashMap<LanguageCommand, String>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceCommand>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateCommand>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateItemCommand {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: String,
    pub title: HashMap<Language, String>,
    pub description: HashMap<Language, String>,
    pub price: Option<Price>,
    pub state: ItemStateCommand,
    pub url: String,
    pub images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateItemCommand {
    pub shop_name: Option<String>,
    pub title: Option<HashMap<Language, String>>,
    pub description: Option<HashMap<Language, String>>,
    pub price: Option<Price>,
    pub state: Option<ItemStateCommand>,
    pub url: Option<String>,
    pub images: Option<Vec<String>>,
}

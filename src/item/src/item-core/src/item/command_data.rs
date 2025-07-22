use crate::item_state::command_data::ItemStateCommandData;
use common::item_id::ItemKey;
use common::language::command_data::LanguageCommandData;
use common::price::command_data::PriceCommandData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateItemCommandData {
    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub shop_name: String,

    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub title: HashMap<LanguageCommandData, String>,

    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub description: HashMap<LanguageCommandData, String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceCommandData>,

    pub state: ItemStateCommandData,

    pub url: String,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,
}

impl CreateItemCommandData {
    pub fn item_key(&self) -> ItemKey {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateItemCommandData {
    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<HashMap<LanguageCommandData, String>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<HashMap<LanguageCommandData, String>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceCommandData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateCommandData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,
}

impl UpdateItemCommandData {
    pub fn item_key(&self) -> ItemKey {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

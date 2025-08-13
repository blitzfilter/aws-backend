use crate::item_state_command_data::ItemStateCommandData;
use common::language::command_data::LanguageCommandData;
use common::language::data::LocalizedTextData;
use common::price::command_data::PriceCommandData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use common::{has_key::HasKey, item_id::ItemKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateItemCommandData {
    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub shop_name: String,

    pub native_title: LocalizedTextData,

    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub other_title: HashMap<LanguageCommandData, String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub native_description: Option<LocalizedTextData>,

    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub other_description: HashMap<LanguageCommandData, String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceCommandData>,

    pub state: ItemStateCommandData,

    pub url: String,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,
}

impl HasKey for CreateItemCommandData {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
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
    pub price: Option<PriceCommandData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateCommandData>,
}

impl HasKey for UpdateItemCommandData {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

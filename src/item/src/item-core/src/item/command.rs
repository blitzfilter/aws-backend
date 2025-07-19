use crate::item_state::domain::ItemState;
use common::language::domain::Language;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateItemCommand {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: String,
    pub title: HashMap<Language, String>,
    pub description: HashMap<Language, String>,
    pub price: Option<Price>,
    pub state: ItemState,
    pub url: String,
    pub images: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateItemCommand {
    pub shop_name: Option<String>,
    pub title: Option<HashMap<Language, String>>,
    pub description: Option<HashMap<Language, String>>,
    pub price: Option<Price>,
    pub state: Option<ItemState>,
    pub url: Option<String>,
    pub images: Option<Vec<String>>,
}

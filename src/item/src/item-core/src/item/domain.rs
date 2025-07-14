use crate::item_state::domain::ItemState;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::language::domain::Language;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub item_id: ItemId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: String,
    pub title: HashMap<Language, String>,
    pub description: HashMap<Language, String>,
    pub price: Option<Price>,
    pub state: ItemState,
    pub url: String,
    pub images: Vec<String>,
    pub hash: String,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

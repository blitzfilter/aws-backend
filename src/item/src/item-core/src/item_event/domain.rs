use crate::item::hash::ItemHash;
use crate::item_state::domain::ItemState;
use common::event::Event;
use common::item_id::ItemId;
use common::language::domain::Language;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;

pub type ItemEvent = Event<ItemId, ItemEventPayload>;

#[derive(Debug, Clone, PartialEq)]
pub enum ItemEventPayload {
    Created(Box<ItemCreatedEventPayload>),
    StateListed,
    StateAvailable,
    StateReserved,
    StateSold,
    StateRemoved,
    PriceDropped(ItemPriceDroppedEventPayload),
    PriceIncreased(ItemPriceIncreasedEventPayload),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemCreatedEventPayload {
    pub(crate) shop_id: ShopId,
    pub(crate) shops_item_id: ShopsItemId,
    pub(crate) shop_name: String,
    pub(crate) title: HashMap<Language, String>,
    pub(crate) description: HashMap<Language, String>,
    pub(crate) price: Option<Price>,
    pub(crate) state: ItemState,
    pub(crate) url: String,
    pub(crate) images: Vec<String>,
    pub(crate) hash: ItemHash,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemPriceDroppedEventPayload {
    pub(crate) price: Price,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemPriceIncreasedEventPayload {
    pub(crate) price: Price,
}

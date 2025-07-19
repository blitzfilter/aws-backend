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
    Created(ItemCreatedEventPayload),
    StateListed(ItemStateChangeEventPayload),
    StateAvailable(ItemStateChangeEventPayload),
    StateReserved(ItemStateChangeEventPayload),
    StateSold(ItemStateChangeEventPayload),
    StateRemoved(ItemStateChangeEventPayload),
    PriceDiscovered(ItemPriceChangeEventPayload),
    PriceDropped(ItemPriceChangeEventPayload),
    PriceIncreased(ItemPriceChangeEventPayload),
}

pub trait ItemCommonEventPayload {
    fn shop_id(&self) -> &ShopId;
    fn shops_item_id(&self) -> &ShopsItemId;
}

impl ItemCommonEventPayload for ItemEventPayload {
    fn shop_id(&self) -> &ShopId {
        match self {
            ItemEventPayload::Created(payload) => payload.shop_id(),
            ItemEventPayload::StateListed(payload) => payload.shop_id(),
            ItemEventPayload::StateAvailable(payload) => payload.shop_id(),
            ItemEventPayload::StateReserved(payload) => payload.shop_id(),
            ItemEventPayload::StateSold(payload) => payload.shop_id(),
            ItemEventPayload::StateRemoved(payload) => payload.shop_id(),
            ItemEventPayload::PriceDiscovered(payload) => payload.shop_id(),
            ItemEventPayload::PriceDropped(payload) => payload.shop_id(),
            ItemEventPayload::PriceIncreased(payload) => payload.shop_id(),
        }
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        match self {
            ItemEventPayload::Created(payload) => payload.shops_item_id(),
            ItemEventPayload::StateListed(payload) => payload.shops_item_id(),
            ItemEventPayload::StateAvailable(payload) => payload.shops_item_id(),
            ItemEventPayload::StateReserved(payload) => payload.shops_item_id(),
            ItemEventPayload::StateSold(payload) => payload.shops_item_id(),
            ItemEventPayload::StateRemoved(payload) => payload.shops_item_id(),
            ItemEventPayload::PriceDiscovered(payload) => payload.shops_item_id(),
            ItemEventPayload::PriceDropped(payload) => payload.shops_item_id(),
            ItemEventPayload::PriceIncreased(payload) => payload.shops_item_id(),
        }
    }
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

impl ItemCommonEventPayload for ItemCreatedEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        &self.shops_item_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemStateChangeEventPayload {
    pub(crate) shop_id: ShopId,
    pub(crate) shops_item_id: ShopsItemId,
}

impl ItemCommonEventPayload for ItemStateChangeEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        &self.shops_item_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemPriceChangeEventPayload {
    pub(crate) shop_id: ShopId,
    pub(crate) shops_item_id: ShopsItemId,
    pub(crate) price: Price,
}

impl ItemCommonEventPayload for ItemPriceChangeEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        &self.shops_item_id
    }
}

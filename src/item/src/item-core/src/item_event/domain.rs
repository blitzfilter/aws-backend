use crate::item::domain::description::Description;
use crate::item::domain::shop_name::ShopName;
use crate::item::domain::title::Title;
use crate::item::hash::ItemHash;
use crate::item_state::domain::ItemState;
use common::currency::domain::Currency;
use common::event::Event;
use common::has::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;
use url::Url;

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

impl HasKey for ItemEventPayload {
    type Key = ItemKey;

    fn key(&self) -> ItemKey {
        ItemKey::new(self.shop_id().clone(), self.shops_item_id().clone())
    }
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
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: ShopName,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub state: ItemState,
    pub url: Url,
    pub images: Vec<Url>,
    pub hash: ItemHash,
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
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub hash: ItemHash,
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
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub price: Price,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub hash: ItemHash,
}

impl ItemCommonEventPayload for ItemPriceChangeEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        &self.shops_item_id
    }
}

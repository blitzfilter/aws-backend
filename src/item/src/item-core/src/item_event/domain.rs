use crate::item::hash::ItemHash;
use crate::item_event::record::ItemEventRecord;
use crate::item_event_type::record::ItemEventTypeRecord;
use crate::item_state::domain::ItemState;
use common::error::missing_field::MissingPersistenceField;
use common::event::Event;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use field::field;
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
    fn item_key(&self) -> ItemKey {
        (self.shop_id().clone(), self.shops_item_id().clone())
    }
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

impl TryFrom<ItemEventRecord> for ItemEvent {
    type Error = MissingPersistenceField;
    fn try_from(record: ItemEventRecord) -> Result<Self, Self::Error> {
        let payload = match record.event_type {
            ItemEventTypeRecord::Created => {
                let price = record
                    .price
                    .ok_or::<MissingPersistenceField>(field!(price@ItemEventRecord).into())?
                    .into();
                let state = record
                    .state
                    .ok_or::<MissingPersistenceField>(field!(state@ItemEventRecord).into())?
                    .into();
                let hash = ItemHash::new(&Some(price), &state);
                let mut title = HashMap::with_capacity(2);
                if let Some(title_en) = record.title_en {
                    title.insert(Language::En, title_en);
                }
                if let Some(title_de) = record.title_de {
                    title.insert(Language::De, title_de);
                }
                if let Some(title_record) = record.title {
                    title.insert(title_record.language.into(), title_record.text);
                }

                let mut description = HashMap::with_capacity(2);
                if let Some(description_en) = record.description_en {
                    description.insert(Language::En, description_en);
                }
                if let Some(description_de) = record.description_de {
                    description.insert(Language::De, description_de);
                }
                if let Some(description_record) = record.description {
                    title.insert(description_record.language.into(), description_record.text);
                }

                ItemEventPayload::Created(ItemCreatedEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                    shop_name: record.shop_name.ok_or::<MissingPersistenceField>(
                        field!(shop_name@ItemEventRecord).into(),
                    )?,
                    title,
                    description,
                    price: Some(price),
                    state,
                    url: record
                        .url
                        .ok_or::<MissingPersistenceField>(field!(url@ItemEventRecord).into())?,
                    images: record.images.unwrap_or_default(),
                    hash,
                })
            }
            ItemEventTypeRecord::StateListed => {
                ItemEventPayload::StateListed(ItemStateChangeEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                })
            }
            ItemEventTypeRecord::StateAvailable => {
                ItemEventPayload::StateAvailable(ItemStateChangeEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                })
            }
            ItemEventTypeRecord::StateReserved => {
                ItemEventPayload::StateReserved(ItemStateChangeEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                })
            }
            ItemEventTypeRecord::StateSold => {
                ItemEventPayload::StateSold(ItemStateChangeEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                })
            }
            ItemEventTypeRecord::StateRemoved => {
                ItemEventPayload::StateRemoved(ItemStateChangeEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                })
            }
            ItemEventTypeRecord::PriceDiscovered => {
                ItemEventPayload::PriceDiscovered(ItemPriceChangeEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                    price: record
                        .price
                        .ok_or::<MissingPersistenceField>(field!(price@ItemEventRecord).into())?
                        .into(),
                })
            }
            ItemEventTypeRecord::PriceDropped => {
                ItemEventPayload::PriceDropped(ItemPriceChangeEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                    price: record
                        .price
                        .ok_or::<MissingPersistenceField>(field!(price@ItemEventRecord).into())?
                        .into(),
                })
            }
            ItemEventTypeRecord::PriceIncreased => {
                ItemEventPayload::PriceIncreased(ItemPriceChangeEventPayload {
                    shop_id: record.shop_id,
                    shops_item_id: record.shops_item_id,
                    price: record
                        .price
                        .ok_or::<MissingPersistenceField>(field!(price@ItemEventRecord).into())?
                        .into(),
                })
            }
        };
        let event = Event {
            aggregate_id: record.item_id,
            event_id: record.event_id,
            timestamp: record.timestamp,
            payload,
        };
        Ok(event)
    }
}

use crate::item::hash::ItemHash;
use crate::item::record::ItemRecord;
use crate::item_event::domain::{
    ItemCreatedEventPayload, ItemEvent, ItemEventPayload, ItemPriceChangeEventPayload,
    ItemStateChangeEventPayload,
};
use crate::item_state::domain::ItemState;
use common::event::Event;
use common::event_id::EventId;
use common::has::HasKey;
use common::item_id::{ItemId, ItemKey};
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
    pub hash: ItemHash,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Item {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        shop_id: ShopId,
        shops_item_id: ShopsItemId,
        shop_name: String,
        title: HashMap<Language, String>,
        description: HashMap<Language, String>,
        price: Option<Price>,
        state: ItemState,
        url: String,
        images: Vec<String>,
    ) -> ItemEvent {
        let hash = ItemHash::new(&price, &state);
        let payload = ItemCreatedEventPayload {
            shop_id,
            shops_item_id,
            shop_name,
            title,
            description,
            price,
            state,
            url,
            images,
            hash,
        };
        ItemEvent {
            aggregate_id: ItemId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(payload),
        }
    }

    pub fn change_state(&mut self, new_state: ItemState) -> Option<ItemEvent> {
        if self.state == new_state {
            None
        } else {
            self.state = new_state;
            self.hash();
            let event_payload_constructor = match new_state {
                ItemState::Listed => ItemEventPayload::StateListed,
                ItemState::Available => ItemEventPayload::StateAvailable,
                ItemState::Reserved => ItemEventPayload::StateReserved,
                ItemState::Sold => ItemEventPayload::StateSold,
                ItemState::Removed => ItemEventPayload::StateRemoved,
            };
            let event = Event {
                aggregate_id: self.item_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload_constructor(ItemStateChangeEventPayload {
                    shop_id: self.shop_id.clone(),
                    shops_item_id: self.shops_item_id.clone(),
                    hash: self.hash.clone(),
                }),
            };
            Some(event)
        }
    }

    pub fn change_price(&mut self, new_price: Price) -> Option<ItemEvent> {
        match self.price {
            None => {
                self.price = Some(new_price);
                self.hash();
                let payload = ItemPriceChangeEventPayload {
                    shop_id: self.shop_id.clone(),
                    shops_item_id: self.shops_item_id.clone(),
                    price: new_price,
                    hash: self.hash.clone(),
                };
                let event = Event {
                    aggregate_id: self.item_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ItemEventPayload::PriceDiscovered(payload),
                };
                Some(event)
            }
            Some(old_price) => {
                self.price = Some(new_price);
                self.hash();
                if old_price.currency == new_price.currency {
                    if old_price.monetary_amount < new_price.monetary_amount {
                        let payload = ItemPriceChangeEventPayload {
                            shop_id: self.shop_id.clone(),
                            shops_item_id: self.shops_item_id.clone(),
                            price: new_price,
                            hash: self.hash.clone(),
                        };
                        let event = Event {
                            aggregate_id: self.item_id,
                            event_id: EventId::new(),
                            timestamp: OffsetDateTime::now_utc(),
                            payload: ItemEventPayload::PriceIncreased(payload),
                        };
                        Some(event)
                    } else if old_price.monetary_amount > new_price.monetary_amount {
                        let payload = ItemPriceChangeEventPayload {
                            shop_id: self.shop_id.clone(),
                            shops_item_id: self.shops_item_id.clone(),
                            price: new_price,
                            hash: self.hash.clone(),
                        };
                        let event = Event {
                            aggregate_id: self.item_id,
                            event_id: EventId::new(),
                            timestamp: OffsetDateTime::now_utc(),
                            payload: ItemEventPayload::PriceDropped(payload),
                        };
                        Some(event)
                    } else {
                        None
                    }
                } else {
                    self.price = Some(new_price);
                    self.hash();
                    let payload = ItemPriceChangeEventPayload {
                        shop_id: self.shop_id.clone(),
                        shops_item_id: self.shops_item_id.clone(),
                        price: new_price,
                        hash: self.hash.clone(),
                    };
                    let event = Event {
                        aggregate_id: self.item_id,
                        event_id: EventId::new(),
                        timestamp: OffsetDateTime::now_utc(),
                        payload: ItemEventPayload::PriceDiscovered(payload),
                    };
                    Some(event)
                }
            }
        }
    }

    fn hash(&mut self) {
        self.hash = ItemHash::new(&self.price, &self.state);
    }
}

impl HasKey for Item {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl From<ItemRecord> for Item {
    fn from(record: ItemRecord) -> Self {
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

        Item {
            item_id: record.item_id,
            event_id: record.event_id,
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id,
            shop_name: record.shop_name,
            title,
            description,
            price: record.price.map(Into::into),
            state: record.state.into(),
            url: record.url,
            images: record.images,
            hash: record.hash,
            created: record.created,
            updated: record.updated,
        }
    }
}

use crate::item::hash::ItemHash;
use crate::item::record::ItemRecord;
use crate::item_event::domain::{
    ItemCreatedEventPayload, ItemEvent, ItemEventPayload, ItemPriceChangeEventPayload,
    ItemStateChangeEventPayload,
};
use crate::item_state::domain::ItemState;
use common::aggregate::{Aggregate, AggregateError};
use common::event::Event;
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
            aggregate_id: ItemId::now(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(payload),
        }
    }

    pub fn list(&mut self) -> ItemEvent {
        self.state = ItemState::Listed;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateListed(ItemStateChangeEventPayload {
                shop_id: self.shop_id.clone(),
                shops_item_id: self.shops_item_id.clone(),
                hash: self.hash.clone(),
            }),
        }
    }

    pub fn available(&mut self) -> ItemEvent {
        self.state = ItemState::Available;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateAvailable(ItemStateChangeEventPayload {
                shop_id: self.shop_id.clone(),
                shops_item_id: self.shops_item_id.clone(),
                hash: self.hash.clone(),
            }),
        }
    }

    pub fn reserve(&mut self) -> ItemEvent {
        self.state = ItemState::Reserved;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateReserved(ItemStateChangeEventPayload {
                shop_id: self.shop_id.clone(),
                shops_item_id: self.shops_item_id.clone(),
                hash: self.hash.clone(),
            }),
        }
    }

    pub fn sell(&mut self) -> ItemEvent {
        self.state = ItemState::Sold;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateSold(ItemStateChangeEventPayload {
                shop_id: self.shop_id.clone(),
                shops_item_id: self.shops_item_id.clone(),
                hash: self.hash.clone(),
            }),
        }
    }

    pub fn remove(&mut self) -> ItemEvent {
        self.state = ItemState::Removed;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateRemoved(ItemStateChangeEventPayload {
                shop_id: self.shop_id.clone(),
                shops_item_id: self.shops_item_id.clone(),
                hash: self.hash.clone(),
            }),
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

// region Aggregate

#[derive(Debug, Clone, thiserror::Error)]
pub enum ItemAggregateError {
    #[error("No events exist to aggregate 'Item'.")]
    Empty,

    #[error("Encountered illegal event '{0:?}' to initialize 'Item'.")]
    IllegalInitialization(ItemEventPayload),

    #[error(
        "Applied illegal event 'ItemEventPayload::Created' with '{0:?}' but 'Item' has already been initialized."
    )]
    CreatedAfterCreated(ItemCreatedEventPayload),
}

impl AggregateError for ItemAggregateError {
    fn empty() -> Self {
        ItemAggregateError::Empty
    }
}

impl Aggregate<ItemEvent> for Item {
    type Error = ItemAggregateError;

    fn init(event: ItemEvent) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        match event.payload {
            ItemEventPayload::Created(payload) => {
                let item = Item {
                    item_id: event.aggregate_id,
                    event_id: event.event_id,
                    shop_id: payload.shop_id,
                    shops_item_id: payload.shops_item_id,
                    shop_name: payload.shop_name,
                    title: payload.title,
                    description: payload.description,
                    price: payload.price,
                    state: payload.state,
                    url: payload.url,
                    images: payload.images,
                    hash: payload.hash,
                    created: event.timestamp,
                    updated: event.timestamp,
                };
                Ok(item)
            }
            other => Err(ItemAggregateError::IllegalInitialization(other)),
        }
    }

    fn apply(&mut self, event: ItemEvent) -> Result<(), Self::Error> {
        match event.payload {
            ItemEventPayload::Created(payload) => {
                Err(ItemAggregateError::CreatedAfterCreated(payload))
            }
            ItemEventPayload::StateListed(payload) => {
                self.state = ItemState::Listed;
                self.hash = payload.hash;
                Ok(())
            }
            ItemEventPayload::StateAvailable(payload) => {
                self.state = ItemState::Available;
                self.hash = payload.hash;
                Ok(())
            }
            ItemEventPayload::StateReserved(payload) => {
                self.state = ItemState::Reserved;
                self.hash = payload.hash;
                Ok(())
            }
            ItemEventPayload::StateSold(payload) => {
                self.state = ItemState::Sold;
                self.hash = payload.hash;
                Ok(())
            }
            ItemEventPayload::StateRemoved(payload) => {
                self.state = ItemState::Removed;
                self.hash = payload.hash;
                Ok(())
            }
            ItemEventPayload::PriceDiscovered(payload) => {
                self.price = Some(payload.price);
                self.hash = payload.hash;
                Ok(())
            }
            ItemEventPayload::PriceDropped(payload) => {
                self.price = Some(payload.price);
                self.hash = payload.hash;
                Ok(())
            }
            ItemEventPayload::PriceIncreased(payload) => {
                self.price = Some(payload.price);
                self.hash = payload.hash;
                Ok(())
            }
        }
    }
}

// endregion

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

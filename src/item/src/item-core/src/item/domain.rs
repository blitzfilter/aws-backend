use crate::item::command::CreateItemCommand;
use crate::item::domain::ItemAggregateError::PriceDiscoveredAfterKnown;
use crate::item::hash::ItemHash;
use crate::item_event::domain::{
    ItemCreatedEventPayload, ItemEvent, ItemEventPayload, ItemPriceDiscoveredEventPayload,
    ItemPriceDroppedEventPayload, ItemPriceIncreasedEventPayload,
};
use crate::item_state::domain::ItemState;
use common::aggregate::{Aggregate, AggregateError};
use common::event::Event;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::price::domain::{FxRate, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub(crate) item_id: ItemId,
    pub(crate) event_id: EventId,
    pub(crate) shop_id: ShopId,
    pub(crate) shops_item_id: ShopsItemId,
    pub(crate) shop_name: String,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) price: Option<Price>,
    pub(crate) state: ItemState,
    pub(crate) url: String,
    pub(crate) images: Vec<String>,
    pub(crate) hash: ItemHash,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}

impl Item {
    pub fn create(cmd: CreateItemCommand, fx_rate: &impl FxRate) -> ItemEvent {
        let price = cmd.price.map(|cmd| Price::from_command(cmd, fx_rate));
        let state = cmd.state.into();
        let hash = ItemHash::new(&price.map(Into::into), &state);
        let payload = ItemCreatedEventPayload {
            shop_id: cmd.shop_id,
            shops_item_id: cmd.shops_item_id,
            shop_name: cmd.shop_name,
            title: cmd.title,
            description: cmd.description,
            price,
            state,
            url: cmd.url,
            images: cmd.images,
            hash,
        };
        ItemEvent {
            aggregate_id: ItemId::now(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(Box::new(payload)),
        }
    }

    pub fn list(&mut self) -> ItemEvent {
        self.state = ItemState::Listed;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateListed,
        }
    }

    pub fn available(&mut self) -> ItemEvent {
        self.state = ItemState::Available;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateAvailable,
        }
    }

    pub fn reserve(&mut self) -> ItemEvent {
        self.state = ItemState::Reserved;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateReserved,
        }
    }

    pub fn sell(&mut self) -> ItemEvent {
        self.state = ItemState::Sold;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateSold,
        }
    }

    pub fn remove(&mut self) -> ItemEvent {
        self.state = ItemState::Removed;
        self.hash();
        Event {
            aggregate_id: self.item_id,
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateRemoved,
        }
    }

    pub fn change_price(&mut self, new_price: Price) -> Option<ItemEvent> {
        match self.price {
            None => {
                self.price = Some(new_price);
                self.hash();
                let payload = ItemPriceDiscoveredEventPayload { price: new_price };
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
                if old_price < new_price {
                    let payload = ItemPriceIncreasedEventPayload { price: new_price };
                    let event = Event {
                        aggregate_id: self.item_id,
                        event_id: EventId::new(),
                        timestamp: OffsetDateTime::now_utc(),
                        payload: ItemEventPayload::PriceIncreased(payload),
                    };
                    Some(event)
                } else if old_price > new_price {
                    let payload = ItemPriceDroppedEventPayload { price: new_price };
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
            }
        }
    }

    fn hash(&mut self) {
        self.hash = ItemHash::new(&self.price.map(Into::into), &self.state);
    }
}

// region Aggregate

#[derive(Debug, Clone, thiserror::Error)]
pub enum ItemAggregateError {
    #[error("No events exist to aggregate 'Item'.")]
    Empty,

    #[error("Encountered illegal event '{0:?}' to initialize 'Item'.")]
    IllegalInitialization(ItemEventPayload),

    #[error("Applied 'ItemEventPayload::Created' but 'Item' has already been initialized.")]
    CreatedAfterCreated(Box<ItemCreatedEventPayload>),

    #[error(
        "Applied 'ItemEventPayload::PriceDiscovered' but 'Item' already has known price '{0:?}'."
    )]
    PriceDiscoveredAfterKnown(Price, ItemPriceDiscoveredEventPayload),
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
            ItemEventPayload::StateListed => {
                self.state = ItemState::Listed;
                self.hash();
                Ok(())
            }
            ItemEventPayload::StateAvailable => {
                self.state = ItemState::Available;
                self.hash();
                Ok(())
            }
            ItemEventPayload::StateReserved => {
                self.state = ItemState::Reserved;
                self.hash();
                Ok(())
            }
            ItemEventPayload::StateSold => {
                self.state = ItemState::Sold;
                self.hash();
                Ok(())
            }
            ItemEventPayload::StateRemoved => {
                self.state = ItemState::Removed;
                self.hash();
                Ok(())
            }
            ItemEventPayload::PriceDiscovered(payload) => match &self.price {
                None => {
                    self.price = Some(payload.price);
                    self.hash();
                    Ok(())
                }
                Some(price) => Err(PriceDiscoveredAfterKnown(*price, payload)),
            },
            ItemEventPayload::PriceDropped(payload) => {
                self.price = Some(payload.price);
                self.hash();
                Ok(())
            }
            ItemEventPayload::PriceIncreased(payload) => {
                self.price = Some(payload.price);
                self.hash();
                Ok(())
            }
        }
    }
}

// endregion

pub mod description;
pub mod shop_name;
pub mod title;

use crate::item::domain::description::Description;
use crate::item::domain::shop_name::ShopName;
use crate::item::domain::title::Title;
use crate::item::hash::ItemHash;
use crate::item::record::ItemRecord;
use crate::item_event::domain::{
    ItemCreatedEventPayload, ItemEvent, ItemEventPayload, ItemPriceChangeEventPayload,
    ItemStateChangeEventPayload,
};
use crate::item_state::domain::ItemState;
use common::currency::domain::Currency;
use common::event::Event;
use common::event_id::EventId;
use common::has::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{FxRate, MonetaryAmount, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub item_id: ItemId,
    pub event_id: EventId,
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
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Item {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        shop_id: ShopId,
        shops_item_id: ShopsItemId,
        shop_name: ShopName,
        native_title: Localized<Language, Title>,
        other_title: HashMap<Language, Title>,
        native_description: Option<Localized<Language, Description>>,
        other_description: HashMap<Language, Description>,
        native_price: Option<Price>,
        other_price: HashMap<Currency, MonetaryAmount>,
        state: ItemState,
        url: Url,
        images: Vec<Url>,
    ) -> ItemEvent {
        let hash = ItemHash::new(&native_price, &state);
        let payload = ItemCreatedEventPayload {
            shop_id,
            shops_item_id,
            shop_name,
            native_title,
            other_title,
            native_description,
            native_price,
            other_price,
            state,
            url,
            images,
            hash,
            other_description,
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

    pub fn change_price(&mut self, new_price: Price, fx_rate: &impl FxRate) -> Option<ItemEvent> {
        let old_price_opt = self.native_price;

        let new_other_price = fx_rate
            .exchange_all(new_price.currency, new_price.monetary_amount)
            .unwrap_or_default();
        self.native_price = Some(new_price);
        self.other_price = new_other_price.clone();
        self.hash();

        let payload = ItemPriceChangeEventPayload {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
            price: new_price,
            other_price: new_other_price,
            hash: self.hash.clone(),
        };

        match old_price_opt {
            None => {
                let event = Event {
                    aggregate_id: self.item_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ItemEventPayload::PriceDiscovered(payload),
                };
                Some(event)
            }
            Some(old_price) => {
                let old_price_for_new_currency = old_price
                    .into_exchanged(fx_rate, new_price.currency)
                    .unwrap_or(old_price);
                if old_price_for_new_currency.monetary_amount < new_price.monetary_amount {
                    let event = Event {
                        aggregate_id: self.item_id,
                        event_id: EventId::new(),
                        timestamp: OffsetDateTime::now_utc(),
                        payload: ItemEventPayload::PriceIncreased(payload),
                    };
                    Some(event)
                } else if old_price_for_new_currency.monetary_amount > new_price.monetary_amount {
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
        self.hash = ItemHash::new(&self.native_price, &self.state);
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
        let mut other_title = HashMap::with_capacity(2);
        if let Some(title_en) = record.title_en {
            other_title.insert(Language::En, title_en.into());
        }
        if let Some(title_de) = record.title_de {
            other_title.insert(Language::De, title_de.into());
        }

        let mut other_description = HashMap::with_capacity(2);
        if let Some(description_en) = record.description_en {
            other_description.insert(Language::En, description_en.into());
        }
        if let Some(description_de) = record.description_de {
            other_description.insert(Language::De, description_de.into());
        }

        let mut other_price = HashMap::with_capacity(2);
        if let Some(price_eur) = record.price_eur {
            other_price.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = record.price_gbp {
            other_price.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = record.price_usd {
            other_price.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = record.price_aud {
            other_price.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = record.price_cad {
            other_price.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = record.price_nzd {
            other_price.insert(Currency::Nzd, price_eur.into());
        }

        Item {
            item_id: record.item_id,
            event_id: record.event_id,
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id,
            shop_name: record.shop_name.into(),
            native_title: Localized::new(
                record.title_native.language.into(),
                record.title_native.text.into(),
            ),
            other_title,
            native_description: record.description_native.map(|text_record| {
                Localized::new(text_record.language.into(), text_record.text.into())
            }),
            other_description,
            native_price: record.price_native.map(Price::from),
            other_price,
            state: record.state.into(),
            url: record.url,
            images: record.images,
            hash: record.hash,
            created: record.created,
            updated: record.updated,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedItemView {
    pub item_id: ItemId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: ShopName,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub state: ItemState,
    pub url: Url,
    pub images: Vec<Url>,
    pub hash: ItemHash,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

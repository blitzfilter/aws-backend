use crate::item_event::domain::{ItemCommonEventPayload, ItemEvent, ItemEventPayload};
use crate::item_event_type::record::ItemEventTypeRecord;
use crate::item_state::record::ItemStateRecord;
use common::event_id::EventId;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::language::record::TextRecord;
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, error};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemEventRecord {
    pub pk: String,

    pub sk: String,

    pub item_id: ItemId,

    pub event_id: EventId,

    pub event_type: ItemEventTypeRecord,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<TextRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<TextRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

impl ItemEventRecord {
    pub fn item_key(&self) -> ItemKey {
        (self.shop_id.clone(), self.shops_item_id.clone())
    }

    pub fn into_item_key(self) -> ItemKey {
        (self.shop_id, self.shops_item_id)
    }
}

impl TryFrom<ItemEvent> for ItemEventRecord {
    type Error = error::Format;
    fn try_from(domain: ItemEvent) -> Result<Self, Self::Error> {
        let shop_id = domain.payload.shop_id();
        let shops_item_id = domain.payload.shops_item_id();
        let pk = format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}");
        let sk = format!("item#event#{}", domain.timestamp.format(&Rfc3339)?);
        let item_id = domain.aggregate_id;
        let event_id = domain.event_id;
        let event_type: ItemEventTypeRecord = (&domain.payload).into();
        let shop_id = shop_id.clone();
        let shops_item_id = shops_item_id.clone();

        match domain.payload {
            ItemEventPayload::Created(payload) => {
                let mut payload = payload;
                let title_de = payload.title.remove(&Language::De);
                let title_en = payload.title.remove(&Language::En);
                let title = payload
                    .title
                    .into_iter()
                    .next()
                    .map(|(lang, s)| TextRecord::new(s, lang.into()));

                let description_de = payload.description.remove(&Language::De);
                let description_en = payload.description.remove(&Language::En);
                let description = payload
                    .description
                    .into_iter()
                    .next()
                    .map(|(lang, s)| TextRecord::new(s, lang.into()));
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: Some(payload.shop_name),
                    title,
                    title_de,
                    title_en,
                    description,
                    description_de,
                    description_en,
                    price: payload.price.map(Into::into),
                    state: Some(payload.state.into()),
                    url: Some(payload.url),
                    images: Some(payload.images),
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::StateListed(_) => {
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: None,
                    title: None,
                    title_de: None,
                    title_en: None,
                    description: None,
                    description_de: None,
                    description_en: None,
                    price: None,
                    state: Some(ItemStateRecord::Listed),
                    url: None,
                    images: None,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::StateAvailable(_) => {
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: None,
                    title: None,
                    title_de: None,
                    title_en: None,
                    description: None,
                    description_de: None,
                    description_en: None,
                    price: None,
                    state: Some(ItemStateRecord::Available),
                    url: None,
                    images: None,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::StateReserved(_) => {
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: None,
                    title: None,
                    title_de: None,
                    title_en: None,
                    description: None,
                    description_de: None,
                    description_en: None,
                    price: None,
                    state: Some(ItemStateRecord::Reserved),
                    url: None,
                    images: None,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::StateSold(_) => {
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: None,
                    title: None,
                    title_de: None,
                    title_en: None,
                    description: None,
                    description_de: None,
                    description_en: None,
                    price: None,
                    state: Some(ItemStateRecord::Sold),
                    url: None,
                    images: None,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::StateRemoved(_) => {
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: None,
                    title: None,
                    title_de: None,
                    title_en: None,
                    description: None,
                    description_de: None,
                    description_en: None,
                    price: None,
                    state: Some(ItemStateRecord::Removed),
                    url: None,
                    images: None,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::PriceDiscovered(payload) => {
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: None,
                    title: None,
                    title_de: None,
                    title_en: None,
                    description: None,
                    description_de: None,
                    description_en: None,
                    price: Some(payload.price.into()),
                    state: None,
                    url: None,
                    images: None,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::PriceDropped(payload) => {
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: None,
                    title: None,
                    title_de: None,
                    title_en: None,
                    description: None,
                    description_de: None,
                    description_en: None,
                    price: Some(payload.price.into()),
                    state: None,
                    url: None,
                    images: None,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
            ItemEventPayload::PriceIncreased(payload) => {
                let record = ItemEventRecord {
                    pk,
                    sk,
                    item_id,
                    event_id,
                    event_type,
                    shop_id,
                    shops_item_id,
                    shop_name: None,
                    title: None,
                    title_de: None,
                    title_en: None,
                    description: None,
                    description_de: None,
                    description_en: None,
                    price: Some(payload.price.into()),
                    state: None,
                    url: None,
                    images: None,
                    timestamp: domain.timestamp,
                };
                Ok(record)
            }
        }
    }
}

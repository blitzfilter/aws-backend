use crate::item::record::ItemRecord;
use crate::item_state::record::ItemStateRecord;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemUpdateRecord {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub event_id: Option<EventId>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_currency: Option<CurrencyRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_amount: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_eur: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hash: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<ItemRecord> for ItemUpdateRecord {
    fn from(value: ItemRecord) -> Self {
        ItemUpdateRecord {
            event_id: Some(value.event_id),
            shop_name: Some(value.shop_name),
            title_de: value.title_de,
            title_en: value.title_en,
            description_de: value.description_de,
            description_en: value.description_en,
            price_currency: value.price_currency,
            price_amount: value.price_amount,
            price_eur: value.price_eur,
            state: Some(value.state),
            url: Some(value.url),
            images: Some(value.images),
            hash: Some(value.hash),
            updated: value.updated,
        }
    }
}

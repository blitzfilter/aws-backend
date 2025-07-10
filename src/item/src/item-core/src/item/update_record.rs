use crate::item::record::ItemRecord;
use crate::item_state::record::ItemStateRecord;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use derive_builder::Builder;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Builder, Debug, Clone, PartialEq, Serialize)]
pub struct ItemUpdateRecord {
    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub event_id: Option<EventId>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub shop_name: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_currency: Option<CurrencyRecord>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_amount: Option<f32>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateRecord>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,

    #[builder(setter(into, strip_option), default)]
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
            price_currency: Some(value.price_currency),
            price_amount: Some(value.price_amount),
            state: Some(value.state),
            url: Some(value.url),
            images: Some(value.images),
            hash: Some(value.hash),
            updated: value.updated,
        }
    }
}

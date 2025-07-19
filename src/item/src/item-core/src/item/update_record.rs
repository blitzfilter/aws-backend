use crate::item::hash::ItemHash;
use crate::item::record::ItemRecord;
use crate::item_state::record::ItemStateRecord;
use common::event_id::EventId;
use common::language::record::TextRecord;
use common::price::record::PriceRecord;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemUpdateRecord {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub event_id: Option<EventId>,

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

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hash: Option<ItemHash>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<ItemRecord> for ItemUpdateRecord {
    fn from(value: ItemRecord) -> Self {
        ItemUpdateRecord {
            event_id: Some(value.event_id),
            shop_name: Some(value.shop_name),
            title: value.title,
            title_de: value.title_de,
            title_en: value.title_en,
            description: value.description,
            description_de: value.description_de,
            description_en: value.description_en,
            price: value.price,
            state: Some(value.state),
            url: Some(value.url),
            images: Some(value.images),
            hash: Some(value.hash),
            updated: value.updated,
        }
    }
}

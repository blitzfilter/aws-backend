use crate::item_state::record::ItemStateRecord;
use common::currency::record::CurrencyRecord;
use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Builder, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRecord {
    #[builder(setter(into))]
    pub pk: String,

    #[builder(setter(into))]
    pub sk: String,

    #[builder(setter(into))]
    pub item_id: String,

    #[builder(setter(into))]
    pub event_id: String,

    #[builder(setter(into))]
    pub shop_id: String,

    #[builder(setter(into))]
    pub shop_name: String,

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

    pub price_currency: CurrencyRecord,

    #[builder(setter(into))]
    pub price_amount: f32,

    #[builder(setter(into))]
    pub state: ItemStateRecord,

    #[builder(setter(into))]
    pub url: String,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,

    #[builder(setter(into))]
    pub hash: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

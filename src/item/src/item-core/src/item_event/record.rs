use crate::item_event_type::record::ItemEventTypeRecord;
use crate::item_state::record::ItemStateRecord;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Builder, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemEventRecord {
    #[builder(setter(into))]
    pub pk: String,

    #[builder(setter(into))]
    pub sk: String,

    #[builder(setter(into))]
    pub item_id: ItemId,

    #[builder(setter(into))]
    pub event_id: EventId,

    #[builder(setter(into))]
    pub event_type: ItemEventTypeRecord,

    #[builder(setter(into, strip_option), default)]
    pub shop_id: ShopId,

    #[builder(setter(into))]
    pub shops_item_id: ShopsItemId,

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
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

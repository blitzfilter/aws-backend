use crate::item_event_type::record::ItemEventTypeRecord;
use crate::item_state::record::ItemStateRecord;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

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
    pub state: Option<ItemStateRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
}

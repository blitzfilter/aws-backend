use crate::item_state::record::ItemStateRecord;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRecord {
    pub pk: String,

    pub sk: String,

    pub gsi_1_pk: String,

    pub gsi_1_sk: String,

    pub item_id: ItemId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub shop_name: String,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

    pub price_currency: CurrencyRecord,

    pub price_amount: f32,

    pub state: ItemStateRecord,

    pub url: String,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,

    pub hash: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

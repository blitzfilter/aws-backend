use crate::item_state::data::ItemStateData;
use common::currency::data::CurrencyData;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::Serialize;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetItemData {
    pub item_id: ItemId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub shop_name: String,

    pub title: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub price: HashMap<CurrencyData, f32>,

    pub state: ItemStateData,

    pub url: String,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,

    pub hash: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

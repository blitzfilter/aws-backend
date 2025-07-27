use crate::item_state::document::ItemStateDocument;
use common::event_id::EventId;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemUpdateDocument {
    pub event_id: EventId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_eur: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_usd: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_gbp: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_aud: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_cad: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_nzd: Option<u64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ItemStateDocument>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_available: Option<bool>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

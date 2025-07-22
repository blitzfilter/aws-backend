use crate::item::hash::ItemHash;
use crate::item_state::record::ItemStateRecord;
use common::event_id::EventId;
use common::price::record::PriceRecord;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemUpdateRecord {
    pub event_id: EventId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateRecord>,

    pub hash: ItemHash,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

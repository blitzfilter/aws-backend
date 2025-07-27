use crate::item::hash::ItemHash;
use crate::item_event::record::ItemEventRecord;
use crate::item_state::record::ItemStateRecord;
use common::event_id::EventId;
use common::price::record::PriceRecord;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemRecordUpdate {
    pub event_id: EventId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateRecord>,

    pub hash: ItemHash,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<ItemEventRecord> for ItemRecordUpdate {
    fn from(event: ItemEventRecord) -> Self {
        ItemRecordUpdate {
            event_id: event.event_id,
            price: event.price,
            state: event.state,
            hash: event.hash,
            updated: event.timestamp,
        }
    }
}

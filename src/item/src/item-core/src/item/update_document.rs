use crate::{item_event::record::ItemEventRecord, item_state::document::ItemStateDocument};
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

impl From<ItemEventRecord> for ItemUpdateDocument {
    fn from(event_record: ItemEventRecord) -> Self {
        let state = event_record.state.map(ItemStateDocument::from);
        ItemUpdateDocument {
            event_id: event_record.event_id,
            price_eur: event_record.price_eur,
            price_usd: event_record.price_usd,
            price_gbp: event_record.price_gbp,
            price_aud: event_record.price_aud,
            price_cad: event_record.price_cad,
            price_nzd: event_record.price_nzd,
            state,
            is_available: state.map(|state| matches!(state, ItemStateDocument::Available)),
            updated: event_record.timestamp,
        }
    }
}

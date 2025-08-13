use common::event_id::EventId;
use common::price::record::PriceRecord;
use item_core::hash::ItemHash;
use serde::Serialize;
use time::OffsetDateTime;

use crate::item_event_record::ItemEventRecord;
use crate::item_state_record::ItemStateRecord;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemRecordUpdate {
    pub event_id: EventId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_native: Option<PriceRecord>,

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
            price_native: event.price_native,
            price_eur: event.price_eur,
            price_usd: event.price_usd,
            price_gbp: event.price_gbp,
            price_aud: event.price_aud,
            price_cad: event.price_cad,
            price_nzd: event.price_nzd,
            state: event.state,
            hash: event.hash,
            updated: event.timestamp,
        }
    }
}

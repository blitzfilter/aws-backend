use crate::item_event::record::ItemEventRecord;
use crate::item_state::document::ItemStateDocument;
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::price::domain::FxRate;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemUpdateDocument {
    pub event_id: EventId,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ItemStateDocument>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_available: Option<bool>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ItemUpdateDocument {
    pub fn from_record(record: ItemEventRecord, fx_rate: &impl FxRate) -> Self {
        let price = record
            .price
            .map(|price| fx_rate.exchange(price.currency.into(), Currency::Eur, price.amount));
        let state = record.state.map(ItemStateDocument::from);
        let is_available = state.map(|state| matches!(state, ItemStateDocument::Available));
        ItemUpdateDocument {
            event_id: record.event_id,
            price,
            state,
            is_available,
            updated: record.timestamp,
        }
    }
}

use crate::item_state::document::ItemStateDocument;
use common::event_id::EventId;
use derive_builder::Builder;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Builder, Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDocumentUpdate {
    #[builder(setter(into))]
    pub event_id: EventId,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<f32>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ItemStateDocument>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_available: Option<bool>,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

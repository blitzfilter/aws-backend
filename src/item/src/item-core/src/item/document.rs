use crate::item_state::document::ItemStateDocument;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Builder, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDocument {
    #[builder(setter(into))]
    pub item_id: ItemId,

    #[builder(setter(into))]
    pub event_id: EventId,

    #[builder(setter(into))]
    pub shop_id: ShopId,

    #[builder(setter(into))]
    pub shops_item_id: ShopsItemId,

    #[builder(setter(into))]
    pub shop_name: String,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

    pub price: f32,

    #[builder(setter(into))]
    pub state: ItemStateDocument,

    #[builder(setter(into))]
    pub is_available: bool,

    #[builder(setter(into))]
    pub url: String,

    #[builder(setter(into, strip_option), default)]
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub image: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ItemDocument {
    pub fn _id(&self) -> String {
        self.item_id.to_string()
    }
}

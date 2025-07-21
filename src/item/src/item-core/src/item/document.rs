use crate::item::record::ItemRecord;
use crate::item_state::document::ItemStateDocument;
use crate::item_state::record::ItemStateRecord;
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::price::domain::FxRate;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDocument {
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

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<f32>,

    pub state: ItemStateDocument,

    pub is_available: bool,

    pub url: String,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ItemDocument {
    pub fn _id(&self) -> ItemId {
        self.item_id
    }

    pub fn from_record(record: ItemRecord, fx_rate: &impl FxRate) -> Self {
        let price = record
            .price
            .map(|price| fx_rate.exchange(price.currency.into(), Currency::Eur, price.amount));
        ItemDocument {
            item_id: record.item_id,
            event_id: record.event_id,
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id,
            shop_name: record.shop_name,
            title_de: record.title_de,
            title_en: record.title_en,
            description_de: record.description_de,
            description_en: record.description_en,
            price,
            state: record.state.into(),
            is_available: matches!(record.state, ItemStateRecord::Available),
            url: record.url,
            images: record.images,
            created: record.created,
            updated: record.updated,
        }
    }
}

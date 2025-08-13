use crate::item_state_data::ItemStateData;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::LocalizedItemView;
use serde::Serialize;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetItemData {
    pub item_id: ItemId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub shop_name: String,

    pub title: LocalizedTextData,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<LocalizedTextData>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<PriceData>,

    pub state: ItemStateData,

    pub url: String,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl HasKey for GetItemData {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl From<LocalizedItemView> for GetItemData {
    fn from(item_view: LocalizedItemView) -> Self {
        GetItemData {
            item_id: item_view.item_id,
            event_id: item_view.event_id,
            shop_id: item_view.shop_id,
            shops_item_id: item_view.shops_item_id,
            shop_name: item_view.shop_name.into(),
            title: item_view.title.into(),
            description: item_view.description.map(LocalizedTextData::from),
            price: item_view.price.map(PriceData::from),
            state: item_view.state.into(),
            url: item_view.url.into(),
            images: item_view.images.into_iter().map(String::from).collect(),
            created: item_view.created,
            updated: item_view.updated,
        }
    }
}

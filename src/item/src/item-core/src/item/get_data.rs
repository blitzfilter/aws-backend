use crate::item::domain::Item;
use crate::item_state::data::ItemStateData;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::language::data::LocalizedTextData;
use common::language::domain::Language;
use common::price::data::PriceData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
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

    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<LocalizedTextData>,

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

impl GetItemData {
    pub fn from_domain_localized(domain: Item, languages: Vec<Language>) -> GetItemData {
        GetItemData {
            item_id: domain.item_id,
            event_id: domain.event_id,
            shop_id: domain.shop_id,
            shops_item_id: domain.shops_item_id,
            shop_name: domain.shop_name,
            title: LocalizedTextData::from_domain_fallbacked(&domain.title, &languages),
            description: LocalizedTextData::from_domain_fallbacked(&domain.description, &languages),
            price: domain.price.map(Into::into),
            state: domain.state.into(),
            url: domain.url,
            images: domain.images,
            created: domain.created,
            updated: domain.updated,
        }
    }
}

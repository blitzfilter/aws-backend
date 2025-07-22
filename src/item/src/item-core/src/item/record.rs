use crate::item::domain::Item;
use crate::item::hash::ItemHash;
use crate::item_state::record::ItemStateRecord;
use common::event_id::EventId;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::language::record::TextRecord;
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemRecord {
    pub pk: String,

    pub sk: String,

    pub gsi_1_pk: String,

    pub gsi_1_sk: String,

    pub item_id: ItemId,

    pub event_id: EventId,

    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub shop_name: String,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<TextRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<TextRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceRecord>,

    pub state: ItemStateRecord,

    pub url: String,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<String>,

    pub hash: ItemHash,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ItemRecord {
    pub fn item_key(&self) -> ItemKey {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl From<Item> for ItemRecord {
    fn from(item: Item) -> Self {
        let mut domain = item;
        let title_de = domain.title.remove(&Language::De);
        let title_en = domain.title.remove(&Language::En);
        let title = domain
            .title
            .into_iter()
            .next()
            .map(|(lang, s)| TextRecord::new(s, lang.into()));

        let description_de = domain.description.remove(&Language::De);
        let description_en = domain.description.remove(&Language::En);
        let description = domain
            .description
            .into_iter()
            .next()
            .map(|(lang, s)| TextRecord::new(s, lang.into()));

        ItemRecord {
            pk: format!(
                "item#shop_id#{}#shops_item_id#{}",
                &domain.shop_id, &domain.shops_item_id
            ),
            sk: "item#materialized".to_string(),
            gsi_1_pk: format!("shop_id#{}", &domain.shop_id),
            gsi_1_sk: format!("updated#{}", &domain.updated),
            item_id: domain.item_id,
            event_id: domain.event_id,
            shop_id: domain.shop_id,
            shops_item_id: domain.shops_item_id,
            shop_name: domain.shop_name,
            title,
            title_de,
            title_en,
            description,
            description_de,
            description_en,
            price: domain.price.map(Into::into),
            state: domain.state.into(),
            url: domain.url,
            images: domain.images,
            hash: domain.hash,
            created: domain.created,
            updated: domain.updated,
        }
    }
}

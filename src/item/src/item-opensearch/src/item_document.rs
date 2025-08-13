use crate::item_state_document::ItemStateDocument;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::item_id::{ItemId, ItemKey};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use common::{event_id::EventId, has_key::HasKey};
use field::field;
use item_dynamodb::item_event_record::ItemEventRecord;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

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

    pub state: ItemStateDocument,

    pub is_available: bool,

    pub url: Url,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<Url>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl ItemDocument {
    pub fn _id(&self) -> ItemId {
        self.item_id
    }
}

impl HasKey for ItemDocument {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl TryFrom<ItemEventRecord> for ItemDocument {
    type Error = PersistenceMappingError;

    fn try_from(event_record: ItemEventRecord) -> Result<Self, Self::Error> {
        let state = event_record
            .state
            .map(ItemStateDocument::from)
            .ok_or_else(|| MissingPersistenceField::new(field!(state@ItemEventRecord)))?;
        let document = ItemDocument {
            item_id: event_record.item_id,
            event_id: event_record.event_id,
            shop_id: event_record.shop_id,
            shops_item_id: event_record.shops_item_id,
            shop_name: event_record
                .shop_name
                .ok_or_else(|| MissingPersistenceField::new(field!(shop_name@ItemEventRecord)))?,
            title_de: event_record.title_de,
            title_en: event_record.title_en,
            description_de: event_record.description_de,
            description_en: event_record.description_en,
            price_eur: event_record.price_eur,
            price_usd: event_record.price_usd,
            price_gbp: event_record.price_gbp,
            price_aud: event_record.price_aud,
            price_cad: event_record.price_cad,
            price_nzd: event_record.price_nzd,
            state,
            url: event_record
                .url
                .ok_or_else(|| MissingPersistenceField::new(field!(url@ItemEventRecord)))?,
            images: event_record.images.unwrap_or_default(),
            created: event_record.timestamp,
            updated: event_record.timestamp,
            is_available: matches!(state, ItemStateDocument::Available),
        };
        Ok(document)
    }
}

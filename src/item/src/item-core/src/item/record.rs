use crate::item::hash::ItemHash;
use crate::item_event::record::ItemEventRecord;
use crate::item_state::record::ItemStateRecord;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::event_id::EventId;
use common::has::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::record::TextRecord;
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use field::field;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known;
use url::Url;

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

    pub title_native: TextRecord,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title_en: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_native: Option<TextRecord>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_de: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description_en: Option<String>,

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

    pub state: ItemStateRecord,

    pub url: Url,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<Url>,

    pub hash: ItemHash,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl HasKey for ItemRecord {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl TryFrom<ItemEventRecord> for ItemRecord {
    type Error = PersistenceMappingError;

    fn try_from(event_record: ItemEventRecord) -> Result<Self, Self::Error> {
        let timestamp_str = event_record.timestamp.format(&well_known::Rfc3339)?;
        let record = ItemRecord {
            pk: event_record.pk,
            sk: "item#materialized".to_string(),
            gsi_1_pk: format!("shop_id#{}", event_record.shop_id),
            gsi_1_sk: format!("updated#{timestamp_str}"),
            item_id: event_record.item_id,
            event_id: event_record.event_id,
            shop_id: event_record.shop_id,
            shops_item_id: event_record.shops_item_id,
            shop_name: event_record
                .shop_name
                .ok_or_else(|| MissingPersistenceField::new(field!(shop_name@ItemEventRecord)))?,
            title_native: event_record.title_native.ok_or_else(|| {
                MissingPersistenceField::new(field!(title_native@ItemEventRecord))
            })?,
            title_de: event_record.title_de,
            title_en: event_record.title_en,
            description_native: event_record.description_native,
            description_de: event_record.description_de,
            description_en: event_record.description_en,
            price_native: event_record.price_native,
            price_eur: event_record.price_eur,
            price_usd: event_record.price_usd,
            price_gbp: event_record.price_gbp,
            price_aud: event_record.price_aud,
            price_cad: event_record.price_cad,
            price_nzd: event_record.price_nzd,
            state: event_record
                .state
                .ok_or_else(|| MissingPersistenceField::new(field!(state@ItemEventRecord)))?,
            url: event_record
                .url
                .ok_or_else(|| MissingPersistenceField::new(field!(url@ItemEventRecord)))?,
            images: event_record.images.unwrap_or_default(),
            hash: event_record.hash,
            created: event_record.timestamp,
            updated: event_record.timestamp,
        };

        Ok(record)
    }
}

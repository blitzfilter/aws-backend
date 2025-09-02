use crate::item_state_document::ItemStateDocument;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::item_id::{ItemId, ItemKey};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use common::{event_id::EventId, has_key::HasKey};
use field::field;
use item_dynamodb::item_event_record::ItemEventRecord;
use item_dynamodb::item_record::ItemRecord;
use item_dynamodb::item_state_record::ItemStateRecord;
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

impl From<ItemRecord> for ItemDocument {
    fn from(record: ItemRecord) -> Self {
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
            price_eur: record.price_eur,
            price_usd: record.price_gbp,
            price_gbp: record.price_gbp,
            price_aud: record.price_aud,
            price_cad: record.price_cad,
            price_nzd: record.price_nzd,
            state: record.state.into(),
            is_available: matches!(record.state, ItemStateRecord::Available),
            url: record.url,
            images: record.images,
            created: record.created,
            updated: record.updated,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, Rng};
    use item_core::description::Description;
    use item_core::shop_name::ShopName;
    use item_core::title::Title;

    impl Dummy<Faker> for ItemDocument {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let state: ItemStateDocument = config.fake_with_rng(rng);
            ItemDocument {
                item_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng::<ShopName, _>(rng).into(),
                title_de: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                description_de: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_en: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                state,
                is_available: matches!(state, ItemStateDocument::Available),
                url: Url::parse(&format!(
                    "https://foo.bar/item/{}",
                    config.fake_with_rng::<u16, _>(rng)
                ))
                .unwrap(),
                images: vec![
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                    Url::parse(&format!(
                        "https://foo.bar/images/{}",
                        config.fake_with_rng::<u16, _>(rng)
                    ))
                    .unwrap(),
                ],
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::item_document::ItemDocument;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_item_document() {
            let _ = Faker.fake::<ItemDocument>();
        }
    }
}

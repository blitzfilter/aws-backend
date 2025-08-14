use std::collections::HashMap;

use crate::item_event_record::ItemEventRecord;
use crate::item_state_record::ItemStateRecord;
use common::currency::domain::Currency;
use common::error::mapping_error::PersistenceMappingError;
use common::error::missing_field::MissingPersistenceField;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::language::record::TextRecord;
use common::localized::Localized;
use common::price::domain::Price;
use common::price::record::PriceRecord;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use field::field;
use item_core::hash::ItemHash;
use item_core::item::Item;
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

impl From<ItemRecord> for Item {
    fn from(record: ItemRecord) -> Self {
        let mut other_title = HashMap::with_capacity(2);
        if let Some(title_en) = record.title_en {
            other_title.insert(Language::En, title_en.into());
        }
        if let Some(title_de) = record.title_de {
            other_title.insert(Language::De, title_de.into());
        }

        let mut other_description = HashMap::with_capacity(2);
        if let Some(description_en) = record.description_en {
            other_description.insert(Language::En, description_en.into());
        }
        if let Some(description_de) = record.description_de {
            other_description.insert(Language::De, description_de.into());
        }

        let mut other_price = HashMap::with_capacity(2);
        if let Some(price_eur) = record.price_eur {
            other_price.insert(Currency::Eur, price_eur.into());
        }
        if let Some(price_eur) = record.price_gbp {
            other_price.insert(Currency::Gbp, price_eur.into());
        }
        if let Some(price_eur) = record.price_usd {
            other_price.insert(Currency::Usd, price_eur.into());
        }
        if let Some(price_eur) = record.price_aud {
            other_price.insert(Currency::Aud, price_eur.into());
        }
        if let Some(price_eur) = record.price_cad {
            other_price.insert(Currency::Cad, price_eur.into());
        }
        if let Some(price_eur) = record.price_nzd {
            other_price.insert(Currency::Nzd, price_eur.into());
        }

        Item {
            item_id: record.item_id,
            event_id: record.event_id,
            shop_id: record.shop_id,
            shops_item_id: record.shops_item_id,
            shop_name: record.shop_name.into(),
            native_title: Localized::new(
                record.title_native.language.into(),
                record.title_native.text.into(),
            ),
            other_title,
            native_description: record.description_native.map(|text_record| {
                Localized::new(text_record.language.into(), text_record.text.into())
            }),
            other_description,
            native_price: record.price_native.map(Price::from),
            other_price,
            state: record.state.into(),
            url: record.url,
            images: record.images,
            hash: record.hash,
            created: record.created,
            updated: record.updated,
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

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::MonetaryAmount;
    use fake::{Dummy, Fake, Faker, Rng};
    use item_core::description::Description;
    use item_core::shop_name::ShopName;
    use item_core::title::Title;

    impl Dummy<Faker> for ItemRecord {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let now = OffsetDateTime::now_utc();
            let now_str = now.format(&well_known::Rfc3339).unwrap();
            let shop_id: ShopId = config.fake_with_rng(rng);
            let shops_item_id: ShopsItemId = config.fake_with_rng(rng);
            let price_native: Option<PriceRecord> =
                Some(config.fake_with_rng::<Price, _>(rng).into());
            let state: ItemStateRecord = config.fake_with_rng(rng);

            ItemRecord {
                pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
                sk: "item#materialized".to_string(),
                gsi_1_pk: format!("shop_id#{shop_id}"),
                gsi_1_sk: format!("updated#{now_str}"),
                item_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: shop_id.clone(),
                shops_item_id: shops_item_id.clone(),
                shop_name: config.fake_with_rng::<ShopName, _>(rng).into(),
                title_native: TextRecord::new(
                    config.fake_with_rng::<Title, _>(rng).to_string(),
                    config.fake_with_rng(rng),
                ),
                title_de: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                title_en: Some(config.fake_with_rng::<Title, _>(rng).to_string()),
                description_native: Some(TextRecord::new(
                    config.fake_with_rng::<Description, _>(rng).to_string(),
                    config.fake_with_rng(rng),
                )),
                description_de: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                description_en: Some(config.fake_with_rng::<Description, _>(rng).to_string()),
                price_native,
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                state,
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
                hash: ItemHash::new(&price_native.map(Price::from), &state.into()),
                created: now,
                updated: now,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::item_record::ItemRecord;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_item_record() {
            let _ = Faker.fake::<ItemRecord>();
        }
    }
}

use common::event_id::EventId;
use common::price::record::PriceRecord;
use item_core::hash::ItemHash;
use serde::Serialize;
use time::OffsetDateTime;

use crate::item_event_record::ItemEventRecord;
use crate::item_state_record::ItemStateRecord;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemRecordUpdate {
    pub event_id: EventId,

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

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateRecord>,

    pub hash: ItemHash,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl From<ItemEventRecord> for ItemRecordUpdate {
    fn from(event: ItemEventRecord) -> Self {
        ItemRecordUpdate {
            event_id: event.event_id,
            price_native: event.price_native,
            price_eur: event.price_eur,
            price_usd: event.price_usd,
            price_gbp: event.price_gbp,
            price_aud: event.price_aud,
            price_cad: event.price_cad,
            price_nzd: event.price_nzd,
            state: event.state,
            hash: event.hash,
            updated: event.timestamp,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{MonetaryAmount, Price};
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ItemRecordUpdate {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let price_native: Option<PriceRecord> =
                Some(config.fake_with_rng::<Price, _>(rng).into());
            let state: ItemStateRecord = config.fake_with_rng(rng);

            ItemRecordUpdate {
                event_id: config.fake_with_rng(rng),
                price_native,
                price_eur: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_usd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_gbp: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_aud: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_cad: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                price_nzd: Some(config.fake_with_rng::<MonetaryAmount, _>(rng).into()),
                state: Some(state),
                hash: ItemHash::new(&price_native.map(Price::from), &state.into()),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::item_update_record::ItemRecordUpdate;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_item_record_update() {
            let _ = Faker.fake::<ItemRecordUpdate>();
        }
    }
}

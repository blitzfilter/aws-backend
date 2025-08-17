use common::item_state::domain::ItemState;
use blake3::Hash;
use common::currency::domain::Currency;
use common::price::domain::{MonetaryAmount, Price};
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Display;
use std::ops::Add;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemHash(Hash);

impl ItemHash {
    pub fn new(price: &Option<Price>, state: &ItemState) -> ItemHash {
        let contribution = price.contribute() + state.contribute();
        ItemHash(blake3::hash(contribution.0.as_bytes()))
    }
}

impl Display for ItemHash {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        self.0.to_hex().fmt(f)
    }
}

impl From<ItemHash> for String {
    fn from(hash: ItemHash) -> Self {
        hash.0.to_hex().to_string()
    }
}

impl Serialize for ItemHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_hex())
    }
}

impl<'de> Deserialize<'de> for ItemHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ItemHashVisitor;

        impl<'de> Visitor<'de> for ItemHashVisitor {
            type Value = ItemHash;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a blake3 hash as a hex string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                blake3::Hash::from_hex(v)
                    .map(ItemHash)
                    .map_err(|_| E::custom("invalid blake3 hex string"))
            }
        }

        deserializer.deserialize_str(ItemHashVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Hash, Serialize, Deserialize)]
pub struct ItemHashContribution(String);
pub trait ItemHashContributor {
    fn contribute(&self) -> ItemHashContribution;
}

impl Add for ItemHashContribution {
    type Output = ItemHashContribution;

    fn add(self, rhs: Self) -> Self::Output {
        ItemHashContribution(self.0 + &rhs.0)
    }
}

impl<T: ItemHashContributor> ItemHashContributor for Option<T> {
    fn contribute(&self) -> ItemHashContribution {
        match self {
            None => ItemHashContribution(String::new()),
            Some(v) => v.contribute(),
        }
    }
}

impl<T: ItemHashContributor, R: ItemHashContributor> ItemHashContributor for (T, R) {
    fn contribute(&self) -> ItemHashContribution {
        self.0.contribute() + self.1.contribute()
    }
}

impl ItemHashContributor for ItemState {
    fn contribute(&self) -> ItemHashContribution {
        match self {
            ItemState::Listed => ItemHashContribution("ItemState::Listed".to_owned()),
            ItemState::Available => ItemHashContribution("ItemState::Available".to_owned()),
            ItemState::Reserved => ItemHashContribution("ItemState::Reserved".to_owned()),
            ItemState::Sold => ItemHashContribution("ItemState::Sold".to_owned()),
            ItemState::Removed => ItemHashContribution("ItemState::Removed".to_owned()),
        }
    }
}

impl ItemHashContributor for Currency {
    fn contribute(&self) -> ItemHashContribution {
        match self {
            Currency::Eur => ItemHashContribution("Currency::Eur".to_string()),
            Currency::Gbp => ItemHashContribution("Currency::Gbp".to_string()),
            Currency::Usd => ItemHashContribution("Currency::Usd".to_string()),
            Currency::Aud => ItemHashContribution("Currency::Aud".to_string()),
            Currency::Cad => ItemHashContribution("Currency::Cad".to_string()),
            Currency::Nzd => ItemHashContribution("Currency::Nzd".to_string()),
        }
    }
}

impl ItemHashContributor for MonetaryAmount {
    fn contribute(&self) -> ItemHashContribution {
        let raw: u64 = (*self).into();
        ItemHashContribution(raw.to_string())
    }
}

impl ItemHashContributor for Price {
    fn contribute(&self) -> ItemHashContribution {
        self.monetary_amount.contribute() + self.currency.contribute()
    }
}

#[cfg(test)]
mod tests {
    use crate::hash::ItemHash;
    use common::{currency::domain::Currency, price::domain::Price};
    use common::item_state::domain::ItemState;

    #[rstest::rstest]
    #[case(
        &Some(Price::new(42u64.into(), Currency::Eur)),
        &ItemState::Available,
        &Some(Price::new(69u64.into(), Currency::Eur)),
        &ItemState::Available
    )]
    #[case(
        &Some(Price::new(8000u64.into(), Currency::Eur)),
        &ItemState::Available,
        &Some(Price::new(8001u64.into(), Currency::Eur)),
        &ItemState::Available
    )]
    #[case(
        &Some(Price::new(1u64.into(), Currency::Eur)),
        &ItemState::Available,
        &Some(Price::new(1u64.into(), Currency::Eur)),
        &ItemState::Sold
    )]
    #[case(
        &Some(Price::new(1000000000u64.into(), Currency::Eur)),
        &ItemState::Sold,
        &Some(Price::new(1000000000u64.into(), Currency::Gbp)),
        &ItemState::Sold
    )]
    #[case(
        &None,
        &ItemState::Reserved,
        &None,
        &ItemState::Removed
    )]
    fn should_compute_different_hash_for_different_inputs(
        #[case] price_1: &Option<Price>,
        #[case] state_1: &ItemState,
        #[case] price_2: &Option<Price>,
        #[case] state_2: &ItemState,
    ) {
        let hash_1 = ItemHash::new(price_1, state_1);
        let hash_2 = ItemHash::new(price_2, state_2);

        assert_ne!(hash_1, hash_2)
    }

    #[rstest::rstest]
    #[case(&Some(Price::new(42u64.into(), Currency::Eur)), &ItemState::Available)]
    #[case(&Some(Price::new(8000u64.into(), Currency::Eur)), &ItemState::Available)]
    #[case(&Some(Price::new(1u64.into(), Currency::Eur)), &ItemState::Available)]
    #[case(&Some(Price::new(1000000000u64.into(), Currency::Eur)), &ItemState::Sold)]
    #[case(&None, &ItemState::Reserved)]
    fn should_compute_same_hash_for_same_inputs(
        #[case] price: &Option<Price>,
        #[case] state: &ItemState,
    ) {
        let hash_1 = ItemHash::new(price, state);
        let hash_2 = ItemHash::new(price, state);

        assert_eq!(hash_1, hash_2)
    }

    #[test]
    fn should_not_change_hashing_behavior_during_development() {
        let expected = "cb08b403582602c8f26eab3927d40f9d55c429d44f95c81f4d4320a2445cdcdd";
        let actual = ItemHash::new(
            &Some(Price::new(42u64.into(), Currency::Eur)),
            &ItemState::Available,
        )
        .to_string();

        assert_eq!(expected, actual);
    }
}

use crate::item::record::ItemRecord;
use crate::item_state::domain::ItemState;
use common::currency::domain::Currency;
use common::has_key::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::price::domain::{MonetaryAmount, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::ops::Add;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemHash(String);

// region impl ItemHash

impl ItemHash {
    pub fn new(price: &Option<Price>, state: &ItemState) -> ItemHash {
        let contribution = price.contribute() + state.contribute();
        ItemHash(blake3::hash(contribution.0.as_bytes()).to_string())
    }
}

impl Display for ItemHash {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        self.0.fmt(f)
    }
}

impl From<ItemHash> for String {
    fn from(hash: ItemHash) -> Self {
        hash.0
    }
}

// endregion

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

// region impl ItemHashContributor

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

// endregion

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemSummaryHash {
    pub item_id: ItemId,
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub hash: ItemHash,
}

impl HasKey for ItemSummaryHash {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl From<ItemRecord> for ItemSummaryHash {
    fn from(value: ItemRecord) -> Self {
        ItemSummaryHash {
            item_id: value.item_id,
            shop_id: value.shop_id,
            shops_item_id: value.shops_item_id,
            hash: value.hash,
        }
    }
}

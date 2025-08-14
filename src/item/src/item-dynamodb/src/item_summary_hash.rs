use common::{
    has_key::HasKey,
    item_id::{ItemId, ItemKey},
    shop_id::ShopId,
    shops_item_id::ShopsItemId,
};
use item_core::hash::ItemHash;
use serde::{Deserialize, Serialize};

use crate::item_record::ItemRecord;

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

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ItemSummaryHash {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ItemSummaryHash {
                item_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                hash: ItemHash::new(&config.fake_with_rng(rng), &config.fake_with_rng(rng)),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::item_summary_hash::ItemSummaryHash;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_get_item_record() {
            let _ = Faker.fake::<ItemSummaryHash>();
        }
    }
}

use crate::item::record::ItemRecord;
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemHash {
    pub item_id: ItemId,
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub hash: String,
}

impl From<ItemRecord> for ItemHash {
    fn from(value: ItemRecord) -> Self {
        ItemHash {
            item_id: value.item_id,
            shop_id: value.shop_id,
            shops_item_id: value.shops_item_id,
            hash: value.hash,
        }
    }
}

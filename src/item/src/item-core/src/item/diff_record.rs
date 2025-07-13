use crate::item::record::ItemRecord;
use crate::item_state::record::ItemStateRecord;
use common::currency::record::CurrencyRecord;
use common::item_id::ItemId;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemDiffRecord {
    pub item_id: ItemId,
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub price_currency: CurrencyRecord,
    pub price_amount: f32,
    pub state: ItemStateRecord,
    pub url: String,
    pub hash: String,
}

impl From<ItemRecord> for ItemDiffRecord {
    fn from(value: ItemRecord) -> Self {
        ItemDiffRecord {
            item_id: value.item_id,
            shop_id: value.shop_id,
            shops_item_id: value.shops_item_id,
            price_currency: value.price_currency,
            price_amount: value.price_amount,
            state: value.state,
            url: value.url,
            hash: value.hash,
        }
    }
}

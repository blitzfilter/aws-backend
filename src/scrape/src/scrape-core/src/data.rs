use common::language::data::LanguageData;
use common::price::data::PriceData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item_state::data::ItemStateData;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct ScrapeItem {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: String,
    pub title: HashMap<LanguageData, String>,
    pub description: HashMap<LanguageData, String>,
    pub price: Option<PriceData>,
    pub state: ItemStateData,
    pub url: String,
    pub images: Vec<String>,
}

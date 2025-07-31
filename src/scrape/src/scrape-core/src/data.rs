use crate::data::ScrapeItemChangeCommandData::{Create, Update};
use common::has::HasKey;
use common::item_id::ItemKey;
use common::language::data::LanguageData;
use common::price::command_data::PriceCommandData;
use common::price::data::PriceData;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::command_data::{CreateItemCommandData, UpdateItemCommandData};
use item_core::item::hash::ItemHash;
use item_core::item_state::data::ItemStateData;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ScrapeItemChangeCommandData {
    Create(CreateItemCommandData),
    Update(UpdateItemCommandData),
}

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

impl HasKey for ScrapeItem {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

impl From<ScrapeItem> for CreateItemCommandData {
    fn from(scrape_item: ScrapeItem) -> Self {
        CreateItemCommandData {
            shop_id: scrape_item.shop_id,
            shops_item_id: scrape_item.shops_item_id,
            shop_name: scrape_item.shop_name,
            title: scrape_item
                .title
                .into_iter()
                .map(|(lang, text)| (lang.into(), text))
                .collect(),
            description: scrape_item
                .description
                .into_iter()
                .map(|(lang, text)| (lang.into(), text))
                .collect(),
            price: scrape_item.price.map(PriceCommandData::from),
            state: scrape_item.state.into(),
            url: scrape_item.url,
            images: scrape_item.images,
        }
    }
}

impl From<ScrapeItem> for UpdateItemCommandData {
    fn from(scrape_item: ScrapeItem) -> Self {
        UpdateItemCommandData {
            shop_id: scrape_item.shop_id,
            shops_item_id: scrape_item.shops_item_id,
            price: scrape_item.price.map(PriceCommandData::from),
            state: Some(scrape_item.state.into()),
        }
    }
}

impl ScrapeItem {
    pub fn into_changes(
        self,
        shop_universe: &HashMap<ShopsItemId, ItemHash>,
    ) -> Option<ScrapeItemChangeCommandData> {
        match shop_universe.get(&self.shops_item_id) {
            None => Some(Create(self.into())),
            Some(previous_hash) => {
                let new_price = self.price.map(Price::from);
                let new_state = self.state.into();
                let new_hash = ItemHash::new(&new_price, &new_state);
                if previous_hash == &new_hash {
                    None
                } else {
                    Some(Update(self.into()))
                }
            }
        }
    }

    pub fn item_key(&self) -> ItemKey {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::{ScrapeItem, ScrapeItemChangeCommandData};
    use common::currency::command_data::CurrencyCommandData;
    use common::currency::data::CurrencyData;
    use common::currency::domain::Currency;
    use common::price::command_data::PriceCommandData;
    use common::price::data::PriceData;
    use common::price::domain::Price;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use item_core::item::command_data::{CreateItemCommandData, UpdateItemCommandData};
    use item_core::item::hash::ItemHash;
    use item_core::item_state::command_data::ItemStateCommandData;
    use item_core::item_state::data::ItemStateData;
    use item_core::item_state::domain::ItemState;
    use std::collections::HashMap;

    #[test]
    fn should_return_create_command_when_item_not_exists_in_universe() {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let scrape_item = ScrapeItem {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "".to_string(),
            title: Default::default(),
            description: Default::default(),
            price: Some(PriceData {
                currency: CurrencyData::Eur,
                amount: 42,
            }),
            state: ItemStateData::Reserved,
            url: "".to_string(),
            images: vec![],
        };
        let expected = CreateItemCommandData {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "".to_string(),
            title: Default::default(),
            description: Default::default(),
            price: Some(PriceCommandData {
                currency: CurrencyCommandData::Eur,
                amount: 42,
            }),
            state: ItemStateCommandData::Reserved,
            url: "".to_string(),
            images: vec![],
        };
        let actual = scrape_item.into_changes(&HashMap::new());

        match actual.unwrap() {
            ScrapeItemChangeCommandData::Create(actual) => {
                assert_eq!(expected, actual);
            }
            ScrapeItemChangeCommandData::Update(_) => {
                panic!("Expected ScrapeItemChangeCommandData::Create")
            }
        }
    }

    #[test]
    fn should_return_update_command_when_item_exists_in_universe_and_price_changed() {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let scrape_item = ScrapeItem {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "".to_string(),
            title: Default::default(),
            description: Default::default(),
            price: Some(PriceData {
                currency: CurrencyData::Eur,
                amount: 120,
            }),
            state: ItemStateData::Listed,
            url: "".to_string(),
            images: vec![],
        };
        let expected = UpdateItemCommandData {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            price: Some(PriceCommandData {
                currency: CurrencyCommandData::Eur,
                amount: 120,
            }),
            state: Some(ItemStateCommandData::Listed),
        };
        let actual = scrape_item.into_changes(&HashMap::from([(
            shops_item_id,
            ItemHash::new(
                &Some(Price {
                    monetary_amount: 100u64.into(),
                    currency: Currency::Eur,
                }),
                &ItemState::Listed,
            ),
        )]));

        match actual.unwrap() {
            ScrapeItemChangeCommandData::Create(_) => {
                panic!("Expected ScrapeItemChangeCommandData::Update")
            }
            ScrapeItemChangeCommandData::Update(actual) => {
                assert_eq!(expected, actual);
            }
        }
    }

    #[test]
    fn should_return_update_command_when_item_exists_in_universe_and_state_changed() {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let scrape_item = ScrapeItem {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "".to_string(),
            title: Default::default(),
            description: Default::default(),
            price: Some(PriceData {
                currency: CurrencyData::Eur,
                amount: 100,
            }),
            state: ItemStateData::Sold,
            url: "".to_string(),
            images: vec![],
        };
        let expected = UpdateItemCommandData {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            price: Some(PriceCommandData {
                currency: CurrencyCommandData::Eur,
                amount: 100,
            }),
            state: Some(ItemStateCommandData::Sold),
        };
        let actual = scrape_item.into_changes(&HashMap::from([(
            shops_item_id,
            ItemHash::new(
                &Some(Price {
                    monetary_amount: 100u64.into(),
                    currency: Currency::Eur,
                }),
                &ItemState::Reserved,
            ),
        )]));

        match actual.unwrap() {
            ScrapeItemChangeCommandData::Create(_) => {
                panic!("Expected ScrapeItemChangeCommandData::Update")
            }
            ScrapeItemChangeCommandData::Update(actual) => {
                assert_eq!(expected, actual);
            }
        }
    }

    #[test]
    fn should_return_no_command_when_item_exists_in_universe_but_nothing_changed() {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let scrape_item = ScrapeItem {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "".to_string(),
            title: Default::default(),
            description: Default::default(),
            price: Some(PriceData {
                currency: CurrencyData::Eur,
                amount: 100,
            }),
            state: ItemStateData::Reserved,
            url: "".to_string(),
            images: vec![],
        };
        let actual = scrape_item.into_changes(&HashMap::from([(
            shops_item_id,
            ItemHash::new(
                &Some(Price {
                    monetary_amount: 100u64.into(),
                    currency: Currency::Eur,
                }),
                &ItemState::Reserved,
            ),
        )]));

        assert!(actual.is_none());
    }
}

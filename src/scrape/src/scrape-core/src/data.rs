use crate::data::ScrapeItemChangeCommandData::{Create, Update};
use common::has_key::HasKey;
use common::item_id::ItemKey;
use common::language::data::{LanguageData, LocalizedTextData};
use common::price::command_data::PriceCommandData;
use common::price::data::PriceData;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::hash::ItemHash;
use item_data::item_state_data::ItemStateData;
use item_service::item_command_data::{CreateItemCommandData, UpdateItemCommandData};
use item_service::item_state_command_data::ItemStateCommandData;
use serde::Serialize;
use std::collections::HashMap;
use url::Url;

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum ScrapeItemChangeCommandData {
    Create(CreateItemCommandData),
    Update(UpdateItemCommandData),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScrapeItem {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: String,
    pub native_title: LocalizedTextData,
    pub other_title: HashMap<LanguageData, String>,
    pub native_description: Option<LocalizedTextData>,
    pub other_description: HashMap<LanguageData, String>,
    pub price: Option<PriceData>,
    pub state: ItemStateData,
    pub url: Url,
    pub images: Vec<Url>,
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

fn state_data_into_cmd_data(data: ItemStateData) -> ItemStateCommandData {
    match data {
        ItemStateData::Listed => ItemStateCommandData::Listed,
        ItemStateData::Available => ItemStateCommandData::Available,
        ItemStateData::Reserved => ItemStateCommandData::Reserved,
        ItemStateData::Sold => ItemStateCommandData::Sold,
        ItemStateData::Removed => ItemStateCommandData::Removed,
    }
}

impl From<ScrapeItem> for CreateItemCommandData {
    fn from(scrape_item: ScrapeItem) -> Self {
        CreateItemCommandData {
            shop_id: scrape_item.shop_id,
            shops_item_id: scrape_item.shops_item_id,
            shop_name: scrape_item.shop_name,
            native_title: scrape_item.native_title,
            other_title: scrape_item
                .other_title
                .into_iter()
                .map(|(lang, text)| (lang.into(), text))
                .collect(),
            native_description: scrape_item.native_description,
            other_description: scrape_item
                .other_description
                .into_iter()
                .map(|(lang, text)| (lang.into(), text))
                .collect(),
            price: scrape_item.price.map(PriceCommandData::from),
            state: state_data_into_cmd_data(scrape_item.state),
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
            state: Some(state_data_into_cmd_data(scrape_item.state)),
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
    use common::language::data::{LanguageData, LocalizedTextData};
    use common::price::command_data::PriceCommandData;
    use common::price::data::PriceData;
    use common::price::domain::Price;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use item_core::hash::ItemHash;
    use common::item_state::domain::ItemState;
    use item_data::item_state_data::ItemStateData;
    use item_service::item_command_data::{CreateItemCommandData, UpdateItemCommandData};
    use item_service::item_state_command_data::ItemStateCommandData;
    use std::collections::HashMap;
    use url::Url;

    #[test]
    fn should_return_create_command_when_item_not_exists_in_universe() {
        let shop_id = ShopId::new();
        let shops_item_id = ShopsItemId::new();
        let scrape_item = ScrapeItem {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "".to_string(),
            native_title: LocalizedTextData {
                text: "boop".to_string(),
                language: LanguageData::De,
            },
            other_title: Default::default(),
            native_description: None,
            other_description: Default::default(),
            price: Some(PriceData {
                currency: CurrencyData::Eur,
                amount: 42,
            }),
            state: ItemStateData::Reserved,
            url: Url::parse("https://foo.bar").unwrap(),
            images: vec![],
        };
        let expected = CreateItemCommandData {
            shop_id: shop_id.clone(),
            shops_item_id: shops_item_id.clone(),
            shop_name: "".to_string(),
            native_title: LocalizedTextData {
                text: "boop".to_string(),
                language: LanguageData::De,
            },
            other_title: Default::default(),
            native_description: None,
            other_description: Default::default(),
            price: Some(PriceCommandData {
                currency: CurrencyCommandData::Eur,
                amount: 42,
            }),
            state: ItemStateCommandData::Reserved,
            url: Url::parse("https://foo.bar").unwrap(),
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
            native_title: LocalizedTextData {
                text: "boop".to_string(),
                language: LanguageData::De,
            },
            other_title: Default::default(),
            native_description: None,
            other_description: Default::default(),
            price: Some(PriceData {
                currency: CurrencyData::Eur,
                amount: 120,
            }),
            state: ItemStateData::Listed,
            url: Url::parse("https://foo.bar").unwrap(),
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
            native_title: LocalizedTextData {
                text: "boop".to_string(),
                language: LanguageData::De,
            },
            other_title: Default::default(),
            native_description: None,
            other_description: Default::default(),
            price: Some(PriceData {
                currency: CurrencyData::Eur,
                amount: 100,
            }),
            state: ItemStateData::Sold,
            url: Url::parse("https://foo.bar").unwrap(),
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
            native_title: LocalizedTextData {
                text: "boop".to_string(),
                language: LanguageData::De,
            },
            other_title: Default::default(),
            native_description: None,
            other_description: Default::default(),
            price: Some(PriceData {
                currency: CurrencyData::Eur,
                amount: 100,
            }),
            state: ItemStateData::Reserved,
            url: Url::parse("https://foo.bar").unwrap(),
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

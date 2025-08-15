use crate::item_state_command_data::ItemStateCommandData;
use common::language::command_data::LanguageCommandData;
use common::language::data::LocalizedTextData;
use common::price::command_data::PriceCommandData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use common::{has_key::HasKey, item_id::ItemKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateItemCommandData {
    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    pub shop_name: String,

    pub native_title: LocalizedTextData,

    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub other_title: HashMap<LanguageCommandData, String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub native_description: Option<LocalizedTextData>,

    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub other_description: HashMap<LanguageCommandData, String>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceCommandData>,

    pub state: ItemStateCommandData,

    pub url: Url,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub images: Vec<Url>,
}

impl HasKey for CreateItemCommandData {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateItemCommandData {
    pub shop_id: ShopId,

    pub shops_item_id: ShopsItemId,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceCommandData>,

    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ItemStateCommandData>,
}

impl HasKey for UpdateItemCommandData {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};
    use url::Url;

    impl Dummy<Faker> for CreateItemCommandData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            CreateItemCommandData {
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
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
            }
        }
    }

    impl Dummy<Faker> for UpdateItemCommandData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            UpdateItemCommandData {
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
                state: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::item_command_data::{CreateItemCommandData, UpdateItemCommandData};
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_create_item_command_data() {
            let _ = Faker.fake::<CreateItemCommandData>();
        }

        #[test]
        fn should_fake_update_item_command_data() {
            let _ = Faker.fake::<UpdateItemCommandData>();
        }
    }
}

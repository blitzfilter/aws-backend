use common::has_key::HasKey;
use common::item_id::ItemKey;
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::Price;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::description::Description;
use item_core::item_state_domain::ItemState;
use item_core::shop_name::ShopName;
use item_core::title::Title;
use std::collections::HashMap;
use url::Url;

use crate::item_command_data::{CreateItemCommandData, UpdateItemCommandData};

#[derive(Debug, Clone, PartialEq)]
pub struct CreateItemCommand {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: ShopName,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub price: Option<Price>,
    pub state: ItemState,
    pub url: Url,
    pub images: Vec<Url>,
}

impl TryFrom<CreateItemCommandData> for CreateItemCommand {
    type Error = url::ParseError;
    fn try_from(data: CreateItemCommandData) -> Result<Self, Self::Error> {
        let cmd = CreateItemCommand {
            shop_id: data.shop_id,
            shops_item_id: data.shops_item_id,
            shop_name: data.shop_name.into(),
            native_title: Localized {
                localization: data.native_title.language.into(),
                payload: data.native_title.text.into(),
            },
            other_title: data
                .other_title
                .into_iter()
                .map(|(language, text)| (language.into(), text.into()))
                .collect(),
            native_description: data.native_description.map(|text| Localized {
                localization: text.language.into(),
                payload: text.text.into(),
            }),
            other_description: data
                .other_description
                .into_iter()
                .map(|(language, text)| (language.into(), text.into()))
                .collect(),
            price: data.price.map(Price::from),
            state: data.state.into(),
            url: Url::parse(&data.url)?,
            images: data
                .images
                .iter()
                .filter_map(|url| Url::parse(url).ok())
                .collect(),
        };
        Ok(cmd)
    }
}

impl HasKey for CreateItemCommand {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct UpdateItemCommand {
    pub price: Option<Price>,
    pub state: Option<ItemState>,
}

impl UpdateItemCommand {
    pub fn is_empty(&self) -> bool {
        self.price.is_none() && self.state.is_none()
    }
}

impl From<UpdateItemCommandData> for UpdateItemCommand {
    fn from(data: UpdateItemCommandData) -> Self {
        UpdateItemCommand {
            price: data.price.map(Price::from),
            state: data.state.map(ItemState::from),
        }
    }
}

use crate::item::command_data::{CreateItemCommandData, UpdateItemCommandData};
use crate::item_state::domain::ItemState;
use common::item_id::ItemKey;
use common::language::domain::Language;
use common::price::domain::{NegativeMonetaryAmountError, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateItemCommand {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: String,
    pub title: HashMap<Language, String>,
    pub description: HashMap<Language, String>,
    pub price: Option<Price>,
    pub state: ItemState,
    pub url: String,
    pub images: Vec<String>,
}

impl TryFrom<CreateItemCommandData> for CreateItemCommand {
    type Error = NegativeMonetaryAmountError;
    fn try_from(data: CreateItemCommandData) -> Result<Self, Self::Error> {
        let cmd = CreateItemCommand {
            shop_id: data.shop_id,
            shops_item_id: data.shops_item_id,
            shop_name: data.shop_name,
            title: data
                .title
                .into_iter()
                .map(|(language, text)| (language.into(), text))
                .collect(),
            description: data
                .description
                .into_iter()
                .map(|(language, text)| (language.into(), text))
                .collect(),
            price: data.price.map(Price::try_from).transpose()?,
            state: data.state.into(),
            url: data.url,
            images: data.images,
        };
        Ok(cmd)
    }
}

impl CreateItemCommand {
    pub fn item_key(&self) -> ItemKey {
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

impl TryFrom<UpdateItemCommandData> for UpdateItemCommand {
    type Error = NegativeMonetaryAmountError;
    fn try_from(data: UpdateItemCommandData) -> Result<Self, Self::Error> {
        Ok(UpdateItemCommand {
            price: data.price.map(Price::try_from).transpose()?,
            state: data.state.map(ItemState::from),
        })
    }
}

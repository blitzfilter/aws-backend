use crate::item::command_data::{CreateItemCommandData, UpdateItemCommandData};
use crate::item_state::domain::ItemState;
use common::language::domain::Language;
use common::price::domain::Price;
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

impl From<CreateItemCommandData> for CreateItemCommand {
    fn from(data: CreateItemCommandData) -> Self {
        CreateItemCommand {
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
            price: data.price.map(Price::from),
            state: data.state.into(),
            url: data.url,
            images: data.images,
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

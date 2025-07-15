use crate::item::command::CreateItemCommand;
use crate::item::hash::ItemHash;
use crate::item_state::domain::ItemState;
use common::event_id::EventId;
use common::item_id::ItemId;
use common::language::domain::Language;
use common::price::domain::{FxRate, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub(crate) item_id: ItemId,
    pub(crate) event_id: EventId,
    pub(crate) shop_id: ShopId,
    pub(crate) shops_item_id: ShopsItemId,
    pub(crate) shop_name: String,
    pub(crate) title: HashMap<Language, String>,
    pub(crate) description: HashMap<Language, String>,
    pub(crate) price: Option<Price>,
    pub(crate) state: ItemState,
    pub(crate) url: String,
    pub(crate) images: Vec<String>,
    pub(crate) hash: ItemHash,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}

impl Item {
    pub fn create(cmd: CreateItemCommand, fx_rate: &impl FxRate) -> Self {
        let price = cmd.price.map(|cmd| Price::from_command(cmd, fx_rate));
        let state = cmd.state.into();
        let hash = ItemHash::new(&price, &state);
        Item {
            item_id: ItemId::now(),
            event_id: EventId::new(),
            shop_id: cmd.shop_id,
            shops_item_id: cmd.shops_item_id,
            shop_name: cmd.shop_name,
            title: cmd
                .title
                .into_iter()
                .map(|(lang, title)| (lang.into(), title))
                .collect(),
            description: cmd
                .description
                .into_iter()
                .map(|(lang, title)| (lang.into(), title))
                .collect(),
            price,
            state,
            url: cmd.url,
            images: cmd.images,
            hash,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }
}

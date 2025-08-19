use common::currency::domain::Currency;
use common::event::Event;
use common::has_key::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;
use url::Url;

use crate::description::Description;
use crate::hash::ItemHash;
use crate::shop_name::ShopName;
use crate::title::Title;
use common::item_state::domain::ItemState;

pub type ItemEvent = Event<ItemId, ItemEventPayload>;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ItemEventPayload {
    Created(ItemCreatedEventPayload),
    StateListed(ItemStateChangeEventPayload),
    StateAvailable(ItemStateChangeEventPayload),
    StateReserved(ItemStateChangeEventPayload),
    StateSold(ItemStateChangeEventPayload),
    StateRemoved(ItemStateChangeEventPayload),
    PriceDiscovered(ItemPriceChangeEventPayload),
    PriceDropped(ItemPriceChangeEventPayload),
    PriceIncreased(ItemPriceChangeEventPayload),
}

impl HasKey for ItemEventPayload {
    type Key = ItemKey;

    fn key(&self) -> ItemKey {
        ItemKey::new(self.shop_id().clone(), self.shops_item_id().clone())
    }
}

pub trait ItemCommonEventPayload {
    fn shop_id(&self) -> &ShopId;
    fn shops_item_id(&self) -> &ShopsItemId;
}

impl ItemCommonEventPayload for ItemEventPayload {
    fn shop_id(&self) -> &ShopId {
        match self {
            ItemEventPayload::Created(payload) => payload.shop_id(),
            ItemEventPayload::StateListed(payload) => payload.shop_id(),
            ItemEventPayload::StateAvailable(payload) => payload.shop_id(),
            ItemEventPayload::StateReserved(payload) => payload.shop_id(),
            ItemEventPayload::StateSold(payload) => payload.shop_id(),
            ItemEventPayload::StateRemoved(payload) => payload.shop_id(),
            ItemEventPayload::PriceDiscovered(payload) => payload.shop_id(),
            ItemEventPayload::PriceDropped(payload) => payload.shop_id(),
            ItemEventPayload::PriceIncreased(payload) => payload.shop_id(),
        }
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        match self {
            ItemEventPayload::Created(payload) => payload.shops_item_id(),
            ItemEventPayload::StateListed(payload) => payload.shops_item_id(),
            ItemEventPayload::StateAvailable(payload) => payload.shops_item_id(),
            ItemEventPayload::StateReserved(payload) => payload.shops_item_id(),
            ItemEventPayload::StateSold(payload) => payload.shops_item_id(),
            ItemEventPayload::StateRemoved(payload) => payload.shops_item_id(),
            ItemEventPayload::PriceDiscovered(payload) => payload.shops_item_id(),
            ItemEventPayload::PriceDropped(payload) => payload.shops_item_id(),
            ItemEventPayload::PriceIncreased(payload) => payload.shops_item_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemCreatedEventPayload {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: ShopName,
    pub native_title: Localized<Language, Title>,
    pub other_title: HashMap<Language, Title>,
    pub native_description: Option<Localized<Language, Description>>,
    pub other_description: HashMap<Language, Description>,
    pub native_price: Option<Price>,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub state: ItemState,
    pub url: Url,
    pub images: Vec<Url>,
    pub hash: ItemHash,
}

impl ItemCommonEventPayload for ItemCreatedEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        &self.shops_item_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemStateChangeEventPayload {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub hash: ItemHash,
}

impl ItemCommonEventPayload for ItemStateChangeEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        &self.shops_item_id
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ItemPriceChangeEventPayload {
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub native_price: Price,
    pub other_price: HashMap<Currency, MonetaryAmount>,
    pub hash: ItemHash,
}

impl ItemCommonEventPayload for ItemPriceChangeEventPayload {
    fn shop_id(&self) -> &ShopId {
        &self.shop_id
    }

    fn shops_item_id(&self) -> &ShopsItemId {
        &self.shops_item_id
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::{FixedFxRate, FxRate};
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for ItemCreatedEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let state = config.fake_with_rng(rng);
            ItemCreatedEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                native_title: config.fake_with_rng(rng),
                other_title: config.fake_with_rng(rng),
                native_description: config.fake_with_rng(rng),
                other_description: config.fake_with_rng(rng),
                native_price,
                other_price,
                state,
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
                hash: ItemHash::new(&native_price, &state),
            }
        }
    }

    impl Dummy<Faker> for ItemStateChangeEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let state = config.fake_with_rng(rng);
            ItemStateChangeEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                hash: ItemHash::new(&native_price, &state),
            }
        }
    }

    impl Dummy<Faker> for ItemPriceChangeEventPayload {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Price = config.fake_with_rng(rng);
            let other_price = FixedFxRate()
                .exchange_all(native_price.currency, native_price.monetary_amount)
                .unwrap();
            let state = config.fake_with_rng(rng);
            ItemPriceChangeEventPayload {
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                native_price,
                other_price,
                hash: ItemHash::new(&Some(native_price), &state),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::item_event::{
            ItemCreatedEventPayload, ItemEvent, ItemEventPayload, ItemStateChangeEventPayload,
        };
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_item_created_event_payload() {
            let _ = Faker.fake::<ItemCreatedEventPayload>();
        }

        #[test]
        fn should_fake_item_state_change_event_payload() {
            let _ = Faker.fake::<ItemStateChangeEventPayload>();
        }

        #[test]
        fn should_fake_item_price_change_event_payload() {
            let _ = Faker.fake::<ItemStateChangeEventPayload>();
        }

        #[test]
        fn should_fake_item_event_payload() {
            let _ = Faker.fake::<ItemEventPayload>();
        }

        #[test]
        fn should_fake_item_event() {
            let _ = Faker.fake::<ItemEvent>();
        }
    }
}

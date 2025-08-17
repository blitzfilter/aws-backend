use common::currency::domain::Currency;
use common::event::Event;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::item_id::{ItemId, ItemKey};
use common::language::domain::Language;
use common::localized::Localized;
use common::price::domain::{FxRate, MonetaryAmount, Price};
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

use crate::description::Description;
use crate::hash::ItemHash;
use crate::item_event::{
    ItemCreatedEventPayload, ItemEvent, ItemEventPayload, ItemPriceChangeEventPayload,
    ItemStateChangeEventPayload,
};
use common::item_state::item_state::ItemState;
use crate::shop_name::ShopName;
use crate::title::Title;

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub item_id: ItemId,
    pub event_id: EventId,
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
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl Item {
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        shop_id: ShopId,
        shops_item_id: ShopsItemId,
        shop_name: ShopName,
        native_title: Localized<Language, Title>,
        other_title: HashMap<Language, Title>,
        native_description: Option<Localized<Language, Description>>,
        other_description: HashMap<Language, Description>,
        native_price: Option<Price>,
        other_price: HashMap<Currency, MonetaryAmount>,
        state: ItemState,
        url: Url,
        images: Vec<Url>,
    ) -> ItemEvent {
        let hash = ItemHash::new(&native_price, &state);
        let payload = ItemCreatedEventPayload {
            shop_id,
            shops_item_id,
            shop_name,
            native_title,
            other_title,
            native_description,
            native_price,
            other_price,
            state,
            url,
            images,
            hash,
            other_description,
        };
        ItemEvent {
            aggregate_id: ItemId::new(),
            event_id: EventId::new(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(payload),
        }
    }

    pub fn change_state(&mut self, new_state: ItemState) -> Option<ItemEvent> {
        if self.state == new_state {
            None
        } else {
            self.state = new_state;
            self.hash();
            let event_payload_constructor = match new_state {
                ItemState::Listed => ItemEventPayload::StateListed,
                ItemState::Available => ItemEventPayload::StateAvailable,
                ItemState::Reserved => ItemEventPayload::StateReserved,
                ItemState::Sold => ItemEventPayload::StateSold,
                ItemState::Removed => ItemEventPayload::StateRemoved,
            };
            let event = Event {
                aggregate_id: self.item_id,
                event_id: EventId::new(),
                timestamp: OffsetDateTime::now_utc(),
                payload: event_payload_constructor(ItemStateChangeEventPayload {
                    shop_id: self.shop_id.clone(),
                    shops_item_id: self.shops_item_id.clone(),
                    hash: self.hash,
                }),
            };
            Some(event)
        }
    }

    pub fn change_price(&mut self, new_price: Price, fx_rate: &impl FxRate) -> Option<ItemEvent> {
        let old_price_opt = self.native_price;

        let new_other_price = fx_rate
            .exchange_all(new_price.currency, new_price.monetary_amount)
            .unwrap_or_default();
        self.native_price = Some(new_price);
        self.other_price = new_other_price.clone();
        self.hash();

        let payload = ItemPriceChangeEventPayload {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
            native_price: new_price,
            other_price: new_other_price,
            hash: self.hash,
        };

        match old_price_opt {
            None => {
                let event = Event {
                    aggregate_id: self.item_id,
                    event_id: EventId::new(),
                    timestamp: OffsetDateTime::now_utc(),
                    payload: ItemEventPayload::PriceDiscovered(payload),
                };
                Some(event)
            }
            Some(old_price) => {
                let old_price_for_new_currency = old_price
                    .into_exchanged(fx_rate, new_price.currency)
                    .unwrap_or(old_price);
                if old_price_for_new_currency.monetary_amount < new_price.monetary_amount {
                    let event = Event {
                        aggregate_id: self.item_id,
                        event_id: EventId::new(),
                        timestamp: OffsetDateTime::now_utc(),
                        payload: ItemEventPayload::PriceIncreased(payload),
                    };
                    Some(event)
                } else if old_price_for_new_currency.monetary_amount > new_price.monetary_amount {
                    let event = Event {
                        aggregate_id: self.item_id,
                        event_id: EventId::new(),
                        timestamp: OffsetDateTime::now_utc(),
                        payload: ItemEventPayload::PriceDropped(payload),
                    };
                    Some(event)
                } else {
                    None
                }
            }
        }
    }

    fn hash(&mut self) {
        self.hash = ItemHash::new(&self.native_price, &self.state);
    }
}

impl HasKey for Item {
    type Key = ItemKey;

    fn key(&self) -> Self::Key {
        ItemKey {
            shop_id: self.shop_id.clone(),
            shops_item_id: self.shops_item_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedItemView {
    pub item_id: ItemId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_item_id: ShopsItemId,
    pub shop_name: ShopName,
    pub title: Localized<Language, Title>,
    pub description: Option<Localized<Language, Description>>,
    pub price: Option<Price>,
    pub state: ItemState,
    pub url: Url,
    pub images: Vec<Url>,
    pub hash: ItemHash,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use common::price::domain::FixedFxRate;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for Item {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let other_price = match native_price {
                None => HashMap::new(),
                Some(price) => FixedFxRate()
                    .exchange_all(price.currency, price.monetary_amount)
                    .unwrap(),
            };
            let state = config.fake_with_rng(rng);
            Item {
                item_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    impl Dummy<Faker> for LocalizedItemView {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let native_price: Option<Price> = config.fake_with_rng(rng);
            let state = config.fake_with_rng(rng);
            LocalizedItemView {
                item_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                shop_id: config.fake_with_rng(rng),
                shops_item_id: config.fake_with_rng(rng),
                shop_name: config.fake_with_rng(rng),
                title: config.fake_with_rng(rng),
                description: config.fake_with_rng(rng),
                price: config.fake_with_rng(rng),
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
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::item::{Item, LocalizedItemView};
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_item() {
            let _ = Faker.fake::<Item>();
        }

        #[test]
        fn should_fake_localized_item_view() {
            let _ = Faker.fake::<LocalizedItemView>();
        }
    }
}

#[cfg(test)]
mod tests {
    mod state {
        use crate::hash::ItemHash;
        use crate::item::Item;
        use common::item_state::item_state::ItemState;
        use common::language::domain::Language;
        use common::localized::Localized;
        use time::OffsetDateTime;
        use url::Url;

        #[rstest::rstest]
        #[case::listed(ItemState::Listed, ItemState::Listed)]
        #[case::available(ItemState::Available, ItemState::Available)]
        #[case::reserved(ItemState::Reserved, ItemState::Reserved)]
        #[case::sold(ItemState::Sold, ItemState::Sold)]
        #[case::removed(ItemState::Removed, ItemState::Removed)]
        fn should_return_none_when_state_did_not_change_for_change_state(
            #[case] from_state: ItemState,
            #[case] to_state: ItemState,
        ) {
            let mut item = Item {
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &from_state),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = item.change_state(to_state);

            assert!(actual.is_none());
        }

        #[rstest::rstest]
        #[case::listed(ItemState::Listed, ItemState::Available)]
        #[case::listed(ItemState::Listed, ItemState::Removed)]
        #[case::available(ItemState::Available, ItemState::Reserved)]
        #[case::available(ItemState::Available, ItemState::Sold)]
        #[case::available(ItemState::Available, ItemState::Removed)]
        #[case::reserved(ItemState::Reserved, ItemState::Available)]
        #[case::reserved(ItemState::Reserved, ItemState::Sold)]
        #[case::sold(ItemState::Sold, ItemState::Removed)]
        fn should_return_state_change_when_state_changed_for_change_state(
            #[case] from_state: ItemState,
            #[case] to_state: ItemState,
        ) {
            let mut item = Item {
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &from_state),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = item.change_state(to_state);
            assert!(actual.is_some());
        }

        #[rstest::rstest]
        #[case::listed(ItemState::Listed, ItemState::Available)]
        #[case::listed(ItemState::Listed, ItemState::Removed)]
        #[case::available(ItemState::Available, ItemState::Reserved)]
        #[case::available(ItemState::Available, ItemState::Sold)]
        #[case::available(ItemState::Available, ItemState::Removed)]
        #[case::reserved(ItemState::Reserved, ItemState::Available)]
        #[case::reserved(ItemState::Reserved, ItemState::Sold)]
        #[case::sold(ItemState::Sold, ItemState::Removed)]
        fn should_change_hash_when_state_changed_for_change_state(
            #[case] from_state: ItemState,
            #[case] to_state: ItemState,
        ) {
            let mut item = Item {
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &from_state),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let initial_item = item.clone();

            let _ = item.change_state(to_state).unwrap();
            assert_ne!(initial_item.hash, item.hash);
        }

        #[rstest::rstest]
        #[case::listed(ItemState::Listed, ItemState::Available)]
        #[case::listed(ItemState::Listed, ItemState::Removed)]
        #[case::available(ItemState::Available, ItemState::Reserved)]
        #[case::available(ItemState::Available, ItemState::Sold)]
        #[case::available(ItemState::Available, ItemState::Removed)]
        #[case::reserved(ItemState::Reserved, ItemState::Available)]
        #[case::reserved(ItemState::Reserved, ItemState::Sold)]
        #[case::sold(ItemState::Sold, ItemState::Removed)]
        fn should_change_state_when_state_changed_for_change_state(
            #[case] from_state: ItemState,
            #[case] to_state: ItemState,
        ) {
            let mut item = Item {
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: from_state,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &from_state),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let _ = item.change_state(to_state).unwrap();
            assert_eq!(to_state, item.state);
        }
    }

    mod price {
        use crate::hash::ItemHash;
        use crate::item::Item;
        use crate::item_event::ItemEventPayload;
        use common::item_state::item_state::ItemState;
        use common::currency::domain::Currency;
        use common::language::domain::Language;
        use common::localized::Localized;
        use common::price::domain::{FxRate, MonetaryAmount, Price};
        use time::OffsetDateTime;
        use url::Url;

        struct IdentityFxRate;

        impl FxRate for IdentityFxRate {
            fn exchange(
                &self,
                _from_currency: Currency,
                _to_currency: Currency,
                from_amount: MonetaryAmount,
            ) -> Result<MonetaryAmount, common::price::domain::MonetaryAmountOverflowError>
            {
                Ok(from_amount)
            }
        }

        #[rstest::rstest]
        #[case::eur_zero(Currency::Eur, 0u64.into())]
        #[case::gbp_zero(Currency::Gbp, 0u64.into())]
        #[case::usd_zero(Currency::Usd, 0u64.into())]
        #[case::aud_zero(Currency::Aud, 0u64.into())]
        #[case::cad_zero(Currency::Cad, 0u64.into())]
        #[case::nzd_zero(Currency::Nzd, 0u64.into())]
        #[case::eur_non_zero(Currency::Eur, 42u64.into())]
        #[case::gbp_non_zero(Currency::Gbp, 42u64.into())]
        #[case::usd_non_zero(Currency::Usd, 42u64.into())]
        #[case::aud_non_zero(Currency::Aud, 42u64.into())]
        #[case::cad_non_zero(Currency::Cad, 42u64.into())]
        #[case::nzd_non_zero(Currency::Nzd, 42u64.into())]
        fn should_return_none_when_price_and_currency_did_not_change_for_change_price(
            #[case] currency: Currency,
            #[case] monetary_amount: MonetaryAmount,
        ) {
            let price = Price {
                monetary_amount,
                currency,
            };
            let mut item = Item {
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(price),
                other_price: Default::default(),
                state: ItemState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &ItemState::Listed),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let actual = item.change_price(price, &IdentityFxRate);

            assert!(actual.is_none());
        }

        #[rstest::rstest]
        #[case::eur_zero(Price::new(0u64.into(), Currency::Eur))]
        #[case::gbp_zero(Price::new(0u64.into(), Currency::Gbp))]
        #[case::usd_zero(Price::new(0u64.into(), Currency::Usd))]
        #[case::aud_zero(Price::new(0u64.into(), Currency::Aud))]
        #[case::cad_zero(Price::new(0u64.into(), Currency::Cad))]
        #[case::nzd_zero(Price::new(0u64.into(), Currency::Nzd))]
        #[case::eur_non_zero(Price::new(42u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(42u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(42u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(42u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(42u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(42u64.into(), Currency::Nzd))]
        fn should_discover_price_when_price_changed_from_none_for_change_price(
            #[case] to_price: Price,
        ) {
            let mut item = Item {
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: None,
                other_price: Default::default(),
                state: ItemState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &ItemState::Listed),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let initial_item = item.clone();

            let actual = item.change_price(to_price, &IdentityFxRate).unwrap();

            match actual.payload {
                ItemEventPayload::PriceDiscovered(payload) => {
                    assert_eq!(to_price, payload.native_price);
                    assert_eq!(item.native_price, Some(to_price));
                    assert_ne!(initial_item.hash, item.hash);
                }
                _ => panic!("Expected ItemEventPayload::PriceDiscovered"),
            }
        }

        #[rstest::rstest]
        #[case::eur_non_zero(Price::new(420u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(430u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(440u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(450u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(460u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(477u64.into(), Currency::Nzd))]
        fn should_find_dropped_price_when_price_dropped_for_change_price(#[case] to_price: Price) {
            let mut item = Item {
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(Price::new(700u64.into(), Currency::Eur)),
                other_price: Default::default(),
                state: ItemState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &ItemState::Listed),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let initial_item = item.clone();

            let actual = item.change_price(to_price, &IdentityFxRate).unwrap();

            match actual.payload {
                ItemEventPayload::PriceDropped(payload) => {
                    assert_eq!(to_price, payload.native_price);
                    assert_eq!(item.native_price, Some(to_price));
                    assert_ne!(initial_item.hash, item.hash);
                }
                _ => panic!("Expected ItemEventPayload::PriceDropped"),
            }
        }

        #[rstest::rstest]
        #[case::eur_non_zero(Price::new(420u64.into(), Currency::Eur))]
        #[case::gbp_non_zero(Price::new(430u64.into(), Currency::Gbp))]
        #[case::usd_non_zero(Price::new(440u64.into(), Currency::Usd))]
        #[case::aud_non_zero(Price::new(450u64.into(), Currency::Aud))]
        #[case::cad_non_zero(Price::new(460u64.into(), Currency::Cad))]
        #[case::nzd_non_zero(Price::new(477u64.into(), Currency::Nzd))]
        fn should_find_increased_price_when_price_increased_for_change_price(
            #[case] to_price: Price,
        ) {
            let mut item = Item {
                item_id: Default::default(),
                event_id: Default::default(),
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "Boop".into(),
                native_title: Localized {
                    localization: Language::De,
                    payload: "Boop".into(),
                },
                other_title: Default::default(),
                native_description: None,
                other_description: Default::default(),
                native_price: Some(Price::new(169u64.into(), Currency::Eur)),
                other_price: Default::default(),
                state: ItemState::Listed,
                url: Url::parse("https://example.com").unwrap(),
                images: vec![],
                hash: ItemHash::new(&None, &ItemState::Listed),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            let initial_item = item.clone();

            let actual = item.change_price(to_price, &IdentityFxRate).unwrap();

            match actual.payload {
                ItemEventPayload::PriceIncreased(payload) => {
                    assert_eq!(to_price, payload.native_price);
                    assert_eq!(item.native_price, Some(to_price));
                    assert_ne!(initial_item.hash, item.hash);
                }
                _ => panic!("Expected ItemEventPayload::PriceIncreased"),
            }
        }
    }
}

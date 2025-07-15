use crate::currency::domain::Currency;
use crate::currency::domain::Currency::Eur;
use std::collections::HashMap;

pub trait FxRate {
    fn exchange(&self, from_currency: Currency, to_currency: Currency, from_amount: f32) -> f32;
    fn exchange_for_eur(&self, from_currency: Currency, from_amount: f32) -> f32 {
        self.exchange(from_currency, Eur, from_amount)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Price {
    pub eur_price: f32,
    pub native_currency: Currency,
    pub native_price: f32,
    pub price: HashMap<Currency, f32>,
}

impl Price {
    pub fn change(&mut self, new_native_price: f32, fx_rate: &impl FxRate) {
        self.eur_price = fx_rate.exchange(self.native_currency, Eur, new_native_price);
        self.native_price = new_native_price;
        for (currency, price) in &mut self.price {
            *price = fx_rate.exchange(self.native_currency, *currency, new_native_price);
        }
    }
}

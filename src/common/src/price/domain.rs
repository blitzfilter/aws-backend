use crate::currency::domain::Currency;
use crate::currency::domain::Currency::*;
use std::collections::HashMap;

pub trait FxRate {
    fn exchange(&self, from_currency: Currency, to_currency: Currency, from_amount: f32) -> f32;
    fn exchange_for_eur(&self, from_currency: Currency, from_amount: f32) -> f32 {
        self.exchange(from_currency, Eur, from_amount)
    }
}

/// as of 2025-07-15
#[derive(Default)]
pub struct FixedFxRate();

impl FxRate for FixedFxRate {
    fn exchange(&self, from_currency: Currency, to_currency: Currency, from_amount: f32) -> f32 {
        let rate = match (from_currency, to_currency) {
            (Eur, Eur) => 1.0,
            (Eur, Usd) => 1.167,
            (Eur, Gbp) => 0.8678,
            (Eur, Aud) => 1.778,
            (Eur, Cad) => 1.597,
            (Eur, Nzd) => 1.947,

            (Usd, Eur) => 0.8569,
            (Usd, Gbp) => 0.7437,
            (Usd, Aud) => 1.523,
            (Usd, Cad) => 1.368,
            (Usd, Nzd) => 1.668,
            (Usd, Usd) => 1.0,

            (Gbp, Eur) => 1.152,
            (Gbp, Usd) => 1.344,
            (Gbp, Aud) => 2.049,
            (Gbp, Cad) => 1.840,
            (Gbp, Nzd) => 2.243,
            (Gbp, Gbp) => 1.0,

            (Aud, Eur) => 0.5623,
            (Aud, Usd) => 0.6561,
            (Aud, Gbp) => 0.4880,
            (Aud, Cad) => 0.8982,
            (Aud, Nzd) => 1.095,
            (Aud, Aud) => 1.0,

            (Cad, Eur) => 0.6260,
            (Cad, Usd) => 0.7305,
            (Cad, Gbp) => 0.5433,
            (Cad, Aud) => 1.113,
            (Cad, Nzd) => 1.219,
            (Cad, Cad) => 1.0,

            (Nzd, Eur) => 0.5135,
            (Nzd, Usd) => 0.5993,
            (Nzd, Gbp) => 0.4457,
            (Nzd, Aud) => 0.9132,
            (Nzd, Cad) => 0.8203,
            (Nzd, Nzd) => 1.0,
        };

        from_amount * rate
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

use crate::currency::data::CurrencyData;
use crate::currency::domain::MinorUnitExponent;
use crate::has::Has;
use crate::price::domain::{NegativeMonetaryAmountError, Price};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceData {
    pub currency: CurrencyData,
    pub amount: u64,
}

impl PriceData {
    pub fn new(currency: CurrencyData, amount: u64) -> Self {
        PriceData { currency, amount }
    }

    pub fn new_f64(
        currency: CurrencyData,
        amount: f64,
    ) -> Result<Self, NegativeMonetaryAmountError> {
        if amount < 0f64 {
            Err(NegativeMonetaryAmountError)
        } else {
            let minor_unit_exponent: &MinorUnitExponent = currency.get();
            let scaled = amount * 10f64.powi(minor_unit_exponent.0 as i32);
            let amount = scaled.trunc() as u64;
            Ok(PriceData { currency, amount })
        }
    }
}

impl From<Price> for PriceData {
    fn from(domain: Price) -> Self {
        PriceData {
            currency: domain.currency.into(),
            amount: domain.monetary_amount.into(),
        }
    }
}

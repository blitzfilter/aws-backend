use crate::currency::data::CurrencyData;
use crate::price::domain::Price;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceData {
    pub currency: CurrencyData,
    pub amount: f32,
}

impl From<Price> for PriceData {
    fn from(domain: Price) -> Self {
        PriceData {
            currency: domain.currency.into(),
            amount: domain.monetary_amount.into(),
        }
    }
}

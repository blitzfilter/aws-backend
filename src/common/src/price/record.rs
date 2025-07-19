use crate::currency::record::CurrencyRecord;
use crate::price::domain::Price;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PriceRecord {
    pub currency: CurrencyRecord,
    pub amount: f32,
}

impl From<Price> for PriceRecord {
    fn from(domain: Price) -> Self {
        PriceRecord {
            currency: domain.currency.into(),
            amount: domain.monetary_amount.into(),
        }
    }
}

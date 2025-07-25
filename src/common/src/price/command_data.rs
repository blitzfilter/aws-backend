use crate::currency::command_data::CurrencyCommandData;
use crate::price::data::PriceData;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PriceCommandData {
    pub currency: CurrencyCommandData,
    pub amount: f32,
}

impl From<PriceData> for PriceCommandData {
    fn from(data: PriceData) -> Self {
        PriceCommandData {
            currency: data.currency.into(),
            amount: data.amount,
        }
    }
}

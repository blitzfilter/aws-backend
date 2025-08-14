use crate::currency::command_data::CurrencyCommandData;
use crate::price::data::PriceData;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PriceCommandData {
    pub currency: CurrencyCommandData,
    pub amount: u64,
}

impl From<PriceData> for PriceCommandData {
    fn from(data: PriceData) -> Self {
        PriceCommandData {
            currency: data.currency.into(),
            amount: data.amount,
        }
    }
}

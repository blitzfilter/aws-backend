use crate::currency::command_data::CurrencyCommandData;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceCommandData {
    pub currency: CurrencyCommandData,
    pub price: f32,
}

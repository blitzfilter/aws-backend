use crate::currency::data::CurrencyData;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceData {
    pub currency: CurrencyData,
    pub price: f32,
}

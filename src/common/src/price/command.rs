use crate::currency::command::CurrencyCommand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceCommand {
    pub currency: CurrencyCommand,
    pub price: f32,
}

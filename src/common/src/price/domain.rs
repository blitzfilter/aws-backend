use crate::currency::domain::Currency;

#[derive(Debug, Clone, PartialEq)]
pub struct Price {
    pub currency: Currency,
    pub price: f32,
}

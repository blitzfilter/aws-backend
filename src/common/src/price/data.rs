use crate::currency::data::CurrencyData;
use crate::currency::domain::HasMinorUnitExponent;
use crate::price::domain::{NegativeMonetaryAmountError, Price};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
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
            let minor_unit_exponent = currency.minor_unit_exponent();
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

#[cfg(test)]
mod tests {
    use crate::currency::data::CurrencyData;
    use crate::price::data::PriceData;

    #[rstest::rstest]
    #[case(0.0, 0)]
    #[case(0.42, 42)]
    #[case(6.98, 698)]
    #[case(37.69, 3769)]
    #[case(37.1, 3710)]
    #[case(100.0, 10000)]
    fn should_succeed_from_f64_when_non_negative(#[case] value: f64, #[case] expected_amount: u64) {
        let price = PriceData::new_f64(CurrencyData::Eur, value);
        assert!(price.is_ok());
        assert_eq!(expected_amount, price.unwrap().amount);
    }

    #[rstest::rstest]
    #[case(-0.42)]
    #[case(-6.98)]
    #[case(-37.69)]
    #[case(-37.1)]
    #[case(-100.0)]
    fn should_fail_from_f64_when_negative(#[case] value: f64) {
        let price = PriceData::new_f64(CurrencyData::Eur, value);
        assert!(price.is_err());
    }
}

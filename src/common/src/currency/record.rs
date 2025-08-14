use crate::currency::domain::Currency;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CurrencyRecord {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
}

impl From<Currency> for CurrencyRecord {
    fn from(domain: Currency) -> Self {
        match domain {
            Currency::Eur => CurrencyRecord::Eur,
            Currency::Gbp => CurrencyRecord::Gbp,
            Currency::Usd => CurrencyRecord::Usd,
            Currency::Aud => CurrencyRecord::Aud,
            Currency::Cad => CurrencyRecord::Cad,
            Currency::Nzd => CurrencyRecord::Nzd,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CurrencyRecord;
    use rstest::rstest;

    #[rstest]
    #[case(CurrencyRecord::Eur, "\"EUR\"")]
    #[case(CurrencyRecord::Gbp, "\"GBP\"")]
    #[case(CurrencyRecord::Usd, "\"USD\"")]
    #[case(CurrencyRecord::Aud, "\"AUD\"")]
    #[case(CurrencyRecord::Cad, "\"CAD\"")]
    #[case(CurrencyRecord::Nzd, "\"NZD\"")]
    fn should_serialize_currency_in_screaming_snake_case(
        #[case] currency: CurrencyRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&currency).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"EUR\"", CurrencyRecord::Eur)]
    #[case("\"GBP\"", CurrencyRecord::Gbp)]
    #[case("\"USD\"", CurrencyRecord::Usd)]
    #[case("\"AUD\"", CurrencyRecord::Aud)]
    #[case("\"CAD\"", CurrencyRecord::Cad)]
    #[case("\"NZD\"", CurrencyRecord::Nzd)]
    fn should_deserialize_currency_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: CurrencyRecord,
    ) {
        let actual = serde_json::from_str::<CurrencyRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

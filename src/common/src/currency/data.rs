use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CurrencyData {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
}

#[cfg(test)]
mod tests {
    use super::CurrencyData;
    use rstest::rstest;

    #[rstest]
    #[case(CurrencyData::Eur, "\"EUR\"")]
    #[case(CurrencyData::Gbp, "\"GBP\"")]
    #[case(CurrencyData::Usd, "\"USD\"")]
    #[case(CurrencyData::Aud, "\"AUD\"")]
    #[case(CurrencyData::Cad, "\"CAD\"")]
    #[case(CurrencyData::Nzd, "\"NZD\"")]
    fn should_serialize_currency_according_to_iso_4217(
        #[case] currency: CurrencyData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&currency).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"EUR\"", CurrencyData::Eur)]
    #[case("\"GBP\"", CurrencyData::Gbp)]
    #[case("\"USD\"", CurrencyData::Usd)]
    #[case("\"AUD\"", CurrencyData::Aud)]
    #[case("\"CAD\"", CurrencyData::Cad)]
    #[case("\"NZD\"", CurrencyData::Nzd)]
    fn should_deserialize_currency_according_to_iso_4217(
        #[case] currency: &str,
        #[case] expected: CurrencyData,
    ) {
        let actual = serde_json::from_str::<CurrencyData>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CurrencyCommandData {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
}

#[cfg(test)]
mod tests {
    use super::CurrencyCommandData;
    use rstest::rstest;

    #[rstest]
    #[case(CurrencyCommandData::Eur, "\"EUR\"")]
    #[case(CurrencyCommandData::Gbp, "\"GBP\"")]
    #[case(CurrencyCommandData::Usd, "\"USD\"")]
    #[case(CurrencyCommandData::Aud, "\"AUD\"")]
    #[case(CurrencyCommandData::Cad, "\"CAD\"")]
    #[case(CurrencyCommandData::Nzd, "\"NZD\"")]
    fn should_serialize_currency_according_to_iso_4217(
        #[case] currency: CurrencyCommandData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&currency).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"EUR\"", CurrencyCommandData::Eur)]
    #[case("\"GBP\"", CurrencyCommandData::Gbp)]
    #[case("\"USD\"", CurrencyCommandData::Usd)]
    #[case("\"AUD\"", CurrencyCommandData::Aud)]
    #[case("\"CAD\"", CurrencyCommandData::Cad)]
    #[case("\"NZD\"", CurrencyCommandData::Nzd)]
    fn should_deserialize_currency_according_to_iso_4217(
        #[case] currency: &str,
        #[case] expected: CurrencyCommandData,
    ) {
        let actual = serde_json::from_str::<CurrencyCommandData>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

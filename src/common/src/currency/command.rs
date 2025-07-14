use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CurrencyCommand {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
}

#[cfg(test)]
mod tests {
    use super::CurrencyCommand;
    use rstest::rstest;

    #[rstest]
    #[case(CurrencyCommand::Eur, "\"EUR\"")]
    #[case(CurrencyCommand::Gbp, "\"GBP\"")]
    #[case(CurrencyCommand::Usd, "\"USD\"")]
    #[case(CurrencyCommand::Aud, "\"AUD\"")]
    #[case(CurrencyCommand::Cad, "\"CAD\"")]
    #[case(CurrencyCommand::Nzd, "\"NZD\"")]
    fn should_serialize_currency_according_to_iso_4217(
        #[case] currency: CurrencyCommand,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&currency).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"EUR\"", CurrencyCommand::Eur)]
    #[case("\"GBP\"", CurrencyCommand::Gbp)]
    #[case("\"USD\"", CurrencyCommand::Usd)]
    #[case("\"AUD\"", CurrencyCommand::Aud)]
    #[case("\"CAD\"", CurrencyCommand::Cad)]
    #[case("\"NZD\"", CurrencyCommand::Nzd)]
    fn should_deserialize_currency_according_to_iso_4217(
        #[case] currency: &str,
        #[case] expected: CurrencyCommand,
    ) {
        let actual = serde_json::from_str::<CurrencyCommand>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

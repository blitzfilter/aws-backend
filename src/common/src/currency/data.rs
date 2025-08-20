use crate::currency::domain::{Currency, HasMinorUnitExponent, MinorUnitExponent};
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CurrencyData {
    #[default]
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
}

impl HasMinorUnitExponent for CurrencyData {
    fn minor_unit_exponent(&self) -> MinorUnitExponent {
        match self {
            CurrencyData::Eur => MinorUnitExponent(2),
            CurrencyData::Gbp => MinorUnitExponent(2),
            CurrencyData::Usd => MinorUnitExponent(2),
            CurrencyData::Aud => MinorUnitExponent(2),
            CurrencyData::Cad => MinorUnitExponent(2),
            CurrencyData::Nzd => MinorUnitExponent(2),
        }
    }
}

impl From<Currency> for CurrencyData {
    fn from(domain: Currency) -> Self {
        match domain {
            Currency::Eur => CurrencyData::Eur,
            Currency::Gbp => CurrencyData::Gbp,
            Currency::Usd => CurrencyData::Usd,
            Currency::Aud => CurrencyData::Aud,
            Currency::Cad => CurrencyData::Cad,
            Currency::Nzd => CurrencyData::Nzd,
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{error::ApiError, error_code::BAD_QUERY_PARAMETER_VALUE},
        currency::data::CurrencyData,
    };
    use aws_lambda_events::query_map::QueryMap;

    pub fn extract_currency_query(query: &QueryMap) -> Result<CurrencyData, ApiError> {
        let currency = query
            .first("currency")
            .filter(|str| !str.is_empty())
            .map(|currency| serde_json::from_str::<CurrencyData>(&format!(r#""{currency}""#)))
            .map(|currency_res| {
                currency_res.map_err(|err| {
                    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                        .with_query_field("currency")
                        .with_message(err.to_string())
                })
            })
            .transpose()?
            .unwrap_or_default();

        Ok(currency)
    }

    #[cfg(test)]
    mod tests {
        use crate::api::{
            error::{ApiErrorSource, ApiErrorSourceType},
            error_code::BAD_QUERY_PARAMETER_VALUE,
        };
        use crate::currency::data::CurrencyData;
        use crate::currency::data::api::extract_currency_query;
        use aws_lambda_events::query_map::QueryMap;
        use std::collections::HashMap;

        #[rstest::rstest]
        #[case::eur("EUR", CurrencyData::Eur)]
        #[case::gbp("GBP", CurrencyData::Gbp)]
        #[case::usd("USD", CurrencyData::Usd)]
        #[case::aud("AUD", CurrencyData::Aud)]
        #[case::cad("CAD", CurrencyData::Cad)]
        #[case::nzd("NZD", CurrencyData::Nzd)]
        fn should_extract_currency(#[case] query_value: String, #[case] expected: CurrencyData) {
            let query = QueryMap::from(HashMap::from_iter([("currency".to_string(), query_value)]));

            let actual = extract_currency_query(&query).unwrap();

            assert_eq!(expected, actual);
        }

        #[rstest::rstest]
        #[case("invalid-currency")]
        #[case("boop")]
        #[case("euronen")]
        #[case("dollers")]
        #[case("moneten")]
        #[case("knete")]
        #[case("knöpfe")]
        fn should_400_when_currency_query_param_is_invalid(#[case] query_value: String) {
            let query = QueryMap::from(HashMap::from_iter([("currency".to_string(), query_value)]));

            let actual = extract_currency_query(&query).unwrap_err();

            assert_eq!(400, actual.status);
            assert_eq!(BAD_QUERY_PARAMETER_VALUE, actual.error);
            assert_eq!(
                Some(ApiErrorSource {
                    field: "currency",
                    source_type: ApiErrorSourceType::Query,
                }),
                actual.source
            )
        }
    }
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

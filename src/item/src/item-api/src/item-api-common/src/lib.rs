use aws_lambda_events::apigw::ApiGatewayV2httpRequest;
use common::api::error::ApiError;
use common::api::error_code::{BAD_HEADER_VALUE, BAD_QUERY_PARAMETER_VALUE};
use common::currency::data::CurrencyData;
use common::currency::domain::Currency;
use common::language::data::LanguageData;
use common::language::domain::Language;
use http::HeaderValue;
use http::header::ACCEPT_LANGUAGE;
use lambda_runtime::LambdaEvent;

pub fn extract_languages_header(
    event: &LambdaEvent<ApiGatewayV2httpRequest>,
) -> Result<Vec<Language>, ApiError> {
    let languages = event
        .payload
        .headers
        .get(ACCEPT_LANGUAGE)
        .map(HeaderValue::to_str)
        .map(|header_value_res| {
            header_value_res.map_err(|_| {
                ApiError::bad_request(BAD_HEADER_VALUE).with_header_field(ACCEPT_LANGUAGE.as_str())
            })
        })
        .transpose()?
        .map(accept_language::parse)
        .unwrap_or_default()
        .into_iter()
        .map(|accept_language| {
            serde_json::from_str::<LanguageData>(&format!(r#""{accept_language}""#))
        })
        .filter_map(Result::ok)
        .map(Language::from)
        .collect::<Vec<_>>();

    Ok(languages)
}

pub fn extract_language_header(
    event: &LambdaEvent<ApiGatewayV2httpRequest>,
) -> Result<Language, ApiError> {
    let language = extract_languages_header(event)?
        .into_iter()
        .next()
        .unwrap_or_default();

    Ok(language)
}

pub fn extract_currency_query(
    event: &LambdaEvent<ApiGatewayV2httpRequest>,
) -> Result<Currency, ApiError> {
    let currency = event
        .payload
        .query_string_parameters
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
        .map(Currency::from)
        .unwrap_or_default();

    Ok(currency)
}

#[cfg(test)]
mod tests {
    use crate::extract_currency_query;
    use crate::extract_language_header;
    use crate::extract_languages_header;
    use common::api::error::ApiErrorSource;
    use common::api::error::ApiErrorSourceType;
    use common::api::error_code::BAD_QUERY_PARAMETER_VALUE;
    use common::currency::domain::Currency;
    use common::language::domain::Language::{self, *};
    use http::header::ACCEPT_LANGUAGE;
    use lambda_runtime::LambdaEvent;
    use test_api::ApiGatewayV2httpRequestProxy;

    #[tokio::test]
    #[rstest::rstest]
    #[case("de", &[De])]
    #[case("de-DE", &[De])]
    #[case("en", &[En])]
    #[case("en-US", &[En])]
    #[case("en-GB", &[En])]
    #[case("es", &[Es])]
    #[case("es-ES", &[Es])]
    #[case("de;q=0.9,en;q=0.8", &[De, En])]
    #[case("en-GB,en;q=0.7,de;q=0.6", &[En, En, De])]
    #[case("es-ES;q=0.9,en;q=0.8,de;q=0.7", &[Es, En, De])]
    #[case("en,fr;q=0.5,de;q=0.3,es;q=0.2", &[En, Fr, De, Es])]
    #[case("pt-BR", &[])]
    #[case("ru", &[])]
    #[case("ja", &[])]
    #[case("zh-CN", &[])]
    #[case("ko-KR", &[])]
    #[case("*", &[])]
    #[case("fr-FR; q=0", &[Fr])]
    #[case("", &[])]
    #[case("null", &[])]
    #[case("undefined", &[])]
    #[case("\"en-US\"", &[])]
    #[case("123", &[])]
    #[case("abcdefg", &[])]
    async fn should_extract_languages(
        #[case] accept_language_header_value: &str,
        #[case] expected: &[Language],
    ) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .header(ACCEPT_LANGUAGE.as_str(), accept_language_header_value)
                .build(),
            context: Default::default(),
        };

        let actual = extract_languages_header(&lambda_event).unwrap();

        assert_eq!(expected, actual.as_slice())
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case("de", De)]
    #[case("de-DE", De)]
    #[case("en", En)]
    #[case("en-US", En)]
    #[case("en-GB", En)]
    #[case("es", Es)]
    #[case("es-ES", Es)]
    #[case("de;q=0.9,en;q=0.8", De)]
    #[case("en-GB,en;q=0.7,de;q=0.6", En)]
    #[case("es-ES;q=0.9,en;q=0.8,de;q=0.7", Es)]
    #[case("en,fr;q=0.5,de;q=0.3,es;q=0.2", En)]
    #[case("pt-BR", De)]
    #[case("ru", De)]
    #[case("ja", De)]
    #[case("zh-CN", De)]
    #[case("ko-KR", De)]
    #[case("*", De)]
    #[case("fr-FR; q=0", Fr)]
    #[case("", De)]
    #[case("null", De)]
    #[case("undefined", De)]
    #[case("\"en-US\"", De)]
    #[case("123", De)]
    #[case("abcdefg", De)]
    async fn should_extract_language(
        #[case] accept_language_header_value: &str,
        #[case] expected: Language,
    ) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .header(ACCEPT_LANGUAGE.as_str(), accept_language_header_value)
                .build(),
            context: Default::default(),
        };

        let actual = extract_language_header(&lambda_event).unwrap();

        assert_eq!(expected, actual)
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case::eur("EUR", Currency::Eur)]
    #[case::gbp("GBP", Currency::Gbp)]
    #[case::usd("USD", Currency::Usd)]
    #[case::aud("AUD", Currency::Aud)]
    #[case::cad("CAD", Currency::Cad)]
    #[case::nzd("NZD", Currency::Nzd)]
    async fn should_extract_currency(#[case] query_value: &str, #[case] expected: Currency) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .query_string_parameter("currency", query_value)
                .build(),
            context: Default::default(),
        };

        let actual = extract_currency_query(&lambda_event).unwrap();

        assert_eq!(expected, actual);
    }

    #[tokio::test]
    #[rstest::rstest]
    #[case("invalid-currency")]
    #[case("boop")]
    #[case("euronen")]
    #[case("dollers")]
    #[case("moneten")]
    #[case("knete")]
    #[case("knöpfe")]
    async fn should_400_when_currency_query_param_is_invalid(#[case] query_param_value: &str) {
        let lambda_event = LambdaEvent {
            payload: ApiGatewayV2httpRequestProxy::builder()
                .http_method(http::Method::GET)
                .query_string_parameter("currency", query_param_value)
                .build(),
            context: Default::default(),
        };

        let actual = extract_currency_query(&lambda_event).unwrap_err();

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

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

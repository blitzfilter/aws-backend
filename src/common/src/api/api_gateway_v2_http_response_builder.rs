use crate::language::data::LanguageData;
use aws_lambda_events::apigw::ApiGatewayV2httpResponse;
use aws_lambda_events::encodings::Body;
use http::header::{
    ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_LANGUAGE, CONTENT_TYPE, ETAG, LAST_MODIFIED,
};
use http::{HeaderMap, HeaderName, HeaderValue};
use httpdate::fmt_http_date;
use std::time::SystemTime;
use tracing::error;

pub struct ApiGatewayV2HttpResponseBuilder {
    status_code: i64,
    headers: HeaderMap,
    body: Option<Body>,
    is_base64_encoded: bool,
}

impl ApiGatewayV2HttpResponseBuilder {
    pub fn new(status_code: i64) -> Self {
        Self {
            status_code,
            headers: HeaderMap::new(),
            body: None,
            is_base64_encoded: false,
        }
    }

    pub fn json(status_code: i64) -> Self {
        Self::new(status_code).content_type("application/json")
    }

    pub fn plain(status_code: i64) -> Self {
        Self::new(status_code).content_type("text/plain")
    }

    pub fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
        self
    }

    pub fn content_type(mut self, content_type: &'static str) -> Self {
        self.headers
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        self
    }

    pub fn content_language(mut self, language: LanguageData) -> Self {
        match serde_json::to_value(language) {
            Ok(content_language) => match content_language.as_str() {
                None => {
                    error!(
                        language = ?language,
                        "Failed to serialize LanguageData as JSON-Value-String when setting HTTP Content-Language."
                    );
                }
                Some(content_language_str) => match HeaderValue::from_str(content_language_str) {
                    Ok(header_value) => {
                        self.headers.insert(CONTENT_LANGUAGE, header_value);
                    }
                    Err(err) => {
                        error!(
                            error = %err,
                            language = ?content_language,
                            "Failed to convert serialized LanguageData to HeaderValue when setting HTTP Content-Language."
                        );
                    }
                },
            },
            Err(err) => {
                error!(
                    error = %err,
                    language = ?language,
                    "Failed to serialize LanguageData when setting HTTP Content-Language."
                );
            }
        }
        self
    }

    pub fn try_content_language(self, content_language_opt: Option<LanguageData>) -> Self {
        if let Some(content_language) = content_language_opt {
            self.content_language(content_language)
        } else {
            self
        }
    }

    pub fn e_tag(mut self, e_tag: &str) -> Self {
        match HeaderValue::from_str(e_tag) {
            Ok(e_tag_value) => {
                self.headers.insert(ETAG, e_tag_value);
            }
            Err(err) => {
                error!(
                    error = %err,
                    eTag = %e_tag,
                    "Failed to convert e_tag to HeaderValue when setting HTTP ETag."
                )
            }
        }
        self
    }

    pub fn last_modified(mut self, last_modified_time: impl Into<SystemTime>) -> Self {
        let last_modified = fmt_http_date(last_modified_time.into());
        match HeaderValue::from_str(&last_modified) {
            Ok(last_modified_value) => {
                self.headers.insert(LAST_MODIFIED, last_modified_value);
            }
            Err(err) => {
                error!(
                    error = %err,
                    lastModified = %last_modified,
                    "Failed to convert lastModified to HeaderValue when setting HTTP Last-Modified."
                )
            }
        }
        self
    }

    pub fn body<T: Into<Body>>(mut self, body: T) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn cors(mut self) -> Self {
        self.headers
            .insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        self
    }

    pub fn base64_encoded(mut self, flag: bool) -> Self {
        self.is_base64_encoded = flag;
        self
    }

    pub fn build(self) -> ApiGatewayV2httpResponse {
        ApiGatewayV2httpResponse {
            status_code: self.status_code,
            headers: self.headers,
            multi_value_headers: Default::default(),
            body: self.body,
            is_base64_encoded: self.is_base64_encoded,
            cookies: vec![],
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::api::api_gateway_proxy_response_builder::ApiGatewayV2HttpResponseBuilder;
    use crate::language::data::LanguageData;
    use std::time::SystemTime;

    #[rstest::rstest]
    #[case::minimal_100(ApiGatewayV2HttpResponseBuilder::new(100))]
    #[case::minimal_200(ApiGatewayV2HttpResponseBuilder::new(200))]
    #[case::minimal_300(ApiGatewayV2HttpResponseBuilder::new(300))]
    #[case::minimal_400(ApiGatewayV2HttpResponseBuilder::new(400))]
    #[case::minimal_500(ApiGatewayV2HttpResponseBuilder::new(500))]
    #[case::json(ApiGatewayV2HttpResponseBuilder::json(200))]
    #[case::plain_text(ApiGatewayV2HttpResponseBuilder::plain(200))]
    #[case::content_language(ApiGatewayV2HttpResponseBuilder::new(200).content_language(LanguageData::De))]
    #[case::try_content_language(ApiGatewayV2HttpResponseBuilder::new(200).try_content_language(Some(LanguageData::En)))]
    #[case::e_tag(ApiGatewayV2HttpResponseBuilder::new(200).e_tag("123456"))]
    #[case::last_modified(ApiGatewayV2HttpResponseBuilder::new(200).last_modified(SystemTime::now()))]
    fn should_build_api_gateway_proxy_response(#[case] builder: ApiGatewayV2HttpResponseBuilder) {
        let _ = builder.build();
    }
}

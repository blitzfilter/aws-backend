use crate::language::data::LanguageData;
use aws_lambda_events::apigw::ApiGatewayProxyResponse;
use aws_lambda_events::encodings::Body;
use http::header::{ACCESS_CONTROL_ALLOW_ORIGIN, CONTENT_LANGUAGE, CONTENT_TYPE};
use http::{HeaderMap, HeaderName, HeaderValue};
use tracing::error;

pub struct ApiGatewayProxyResponseBuilder {
    status_code: i64,
    headers: HeaderMap,
    body: Option<Body>,
    is_base64_encoded: bool,
}

impl ApiGatewayProxyResponseBuilder {
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
        match serde_json::to_value(&language) {
            Ok(content_language) => match content_language.as_str() {
                None => {
                    error!(
                        payload = ?language,
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
                            payload = ?content_language,
                            "Failed to convert serialized LanguageData to HeaderValue when setting HTTP Content-Language."
                        );
                    }
                },
            },
            Err(err) => {
                error!(
                    error = %err,
                    payload = ?language,
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

    pub fn build(self) -> ApiGatewayProxyResponse {
        ApiGatewayProxyResponse {
            status_code: self.status_code,
            headers: self.headers,
            multi_value_headers: Default::default(),
            body: self.body,
            is_base64_encoded: self.is_base64_encoded,
        }
    }
}

use aws_lambda_events::apigw::ApiGatewayProxyResponse;
use aws_lambda_events::encodings::Body;
use http::{HeaderMap, HeaderName, HeaderValue};

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
        self.headers.insert(
            HeaderName::from_static("content-type"),
            HeaderValue::from_static(content_type),
        );
        self
    }

    pub fn body<T: Into<Body>>(mut self, body: T) -> Self {
        self.body = Some(body.into());
        self
    }

    pub fn cors(mut self) -> Self {
        self.headers.insert(
            HeaderName::from_static("access-control-allow-origin"),
            HeaderValue::from_static("*"),
        );
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

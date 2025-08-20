use std::collections::HashMap;

use aws_lambda_events::{
    apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpRequestContext},
    query_map::QueryMap,
};
use http::{HeaderMap, Method};
use typed_builder::TypedBuilder;

#[derive(TypedBuilder)]
#[builder(field_defaults(default, setter(strip_option)), build_method(into = ApiGatewayV2httpRequest))]
pub struct ApiGatewayV2httpRequestProxy {
    pub kind: Option<String>,
    pub method_arn: Option<String>,
    #[builder(default = Method::GET, setter(!strip_option))]
    pub http_method: Method,
    pub identity_source: Option<String>,
    pub authorization_token: Option<String>,
    pub resource: Option<String>,
    pub version: Option<String>,
    pub route_key: Option<String>,
    pub raw_path: Option<String>,
    pub raw_query_string: Option<String>,
    pub cookies: Option<Vec<String>>,

    #[builder(
        setter(!strip_option),
        mutators(
            pub fn header(self, key: impl Into<String>, value: impl Into<String>) {
                let mut current: HashMap<String, String> = self
                    .headers
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_str().unwrap().to_owned()))
                    .collect();
                current.insert(key.into(), value.into());
                self.headers = HeaderMap::try_from(&current).unwrap();
            }

            pub fn try_header(self, key: impl Into<String>, value: Option<impl Into<String>>) {
                if let Some(value) = value {
                    let mut current: HashMap<String, String> = self
                        .headers
                        .iter()
                        .map(|(key, value)| (key.to_string(), value.to_str().unwrap().to_owned()))
                        .collect();
                    current.insert(key.into(), value.into());
                    self.headers = HeaderMap::try_from(&current).unwrap();
                }
            }
        ),
        via_mutators
    )]
    pub headers: HeaderMap,

    #[builder(
        setter(!strip_option),
        mutators(
            pub fn query_string_parameter(self, key: impl Into<String>, value: impl Into<String>) {
                let mut current: HashMap<String, String> = self
                    .query_string_parameters
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                current.insert(key.into(), value.into());
                self.query_string_parameters = QueryMap::from(current);
            }

            pub fn try_query_string_parameter(self, key: impl Into<String>, value: Option<impl Into<String>>) {
                if let Some(value) = value {
                    let mut current: HashMap<String, String> = self
                        .query_string_parameters
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    current.insert(key.into(), value.into());
                    self.query_string_parameters = QueryMap::from(current);
                }
            }
        ),
        via_mutators
    )]
    pub query_string_parameters: QueryMap,

    #[builder(
        setter(!strip_option),
        mutators(
            pub fn path_parameter(self, key: impl Into<String>, value: impl Into<String>) {
                self.path_parameters.insert(key.into(), value.into());
            }
        ),
        via_mutators
    )]
    pub path_parameters: HashMap<String, String>,
    #[builder(setter(!strip_option))]
    pub request_context: ApiGatewayV2httpRequestContext,
    #[builder(setter(!strip_option))]
    pub stage_variables: HashMap<String, String>,
    pub body: Option<String>,
    #[builder(setter(!strip_option))]
    pub is_base64_encoded: bool,
}

impl From<ApiGatewayV2httpRequestProxy> for ApiGatewayV2httpRequest {
    fn from(val: ApiGatewayV2httpRequestProxy) -> Self {
        ApiGatewayV2httpRequest {
            kind: val.kind,
            method_arn: val.method_arn,
            http_method: val.http_method,
            identity_source: val.identity_source,
            authorization_token: val.authorization_token,
            resource: val.resource,
            version: val.version,
            route_key: val.route_key,
            raw_path: val.raw_path,
            raw_query_string: val.raw_query_string,
            cookies: val.cookies,
            headers: val.headers,
            query_string_parameters: val.query_string_parameters,
            path_parameters: val.path_parameters,
            request_context: val.request_context,
            stage_variables: val.stage_variables,
            body: val.body,
            is_base64_encoded: val.is_base64_encoded,
        }
    }
}

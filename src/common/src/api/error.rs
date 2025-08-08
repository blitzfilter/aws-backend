use crate::api::api_gateway_v2_http_response_builder::ApiGatewayV2HttpResponseBuilder;
use crate::api::error_code::ApiErrorCode;
use aws_lambda_events::apigw::ApiGatewayV2httpResponse;
use http::StatusCode;
use serde::Serialize;
use std::error::Error;

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub status: u16,

    pub error: ApiErrorCode,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ApiErrorSource>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} - {}", self.status, self.error)?;
        if let Some(msg) = &self.message {
            write!(f, ": {msg}")?;
        }
        Ok(())
    }
}

impl Error for ApiError {}

impl ApiError {
    pub fn new(status: StatusCode, error: ApiErrorCode) -> Self {
        ApiError {
            status: status.as_u16(),
            error,
            source: None,
            message: None,
        }
    }

    pub fn with_source(mut self, field: ApiErrorSource) -> Self {
        self.source = Some(field);
        self
    }

    pub fn with_header_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Header,
        });
        self
    }

    pub fn with_query_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Query,
        });
        self
    }

    pub fn with_path_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Path,
        });
        self
    }
    pub fn with_body_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Body,
        });
        self
    }

    pub fn with_message(mut self, msg: impl Into<String>) -> Self {
        self.message = Some(msg.into());
        self
    }

    pub fn bad_request(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error)
    }

    pub fn unauthorized(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, error)
    }

    pub fn forbidden(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::FORBIDDEN, error)
    }

    pub fn not_found(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::NOT_FOUND, error)
    }

    pub fn conflict(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::CONFLICT, error)
    }

    pub fn unprocessable_entity(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, error)
    }

    pub fn internal_server_error(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error)
    }

    pub fn service_unavailable(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, error)
    }

    pub fn gateway_time_out(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, error)
    }
}

impl From<ApiError> for ApiGatewayV2httpResponse {
    fn from(api_error: ApiError) -> Self {
        ApiGatewayV2HttpResponseBuilder::json(api_error.status.into())
            .body(serde_json::to_string(&api_error).unwrap())
            .cors()
            .build()
    }
}

#[derive(Debug, Serialize)]
pub struct ApiErrorSource {
    pub field: &'static str,

    #[serde(rename = "type")]
    pub source_type: ApiErrorSourceType,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ApiErrorSourceType {
    Header,
    Path,
    Query,
    Body,
}

#[cfg(feature = "dynamodb")]
pub mod dynamodb {
    use crate::api::error::ApiError;
    use crate::api::error_code::{GATEWAY_TIMEOUT, INTERNAL_SERVER_ERROR};
    use aws_sdk_dynamodb::error::SdkError;

    impl<S> From<SdkError<S>> for ApiError {
        fn from(e: SdkError<S>) -> Self {
            match e {
                SdkError::ConstructionFailure(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
                }
                SdkError::TimeoutError(_) => ApiError::gateway_time_out(GATEWAY_TIMEOUT),
                SdkError::DispatchFailure(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
                }
                SdkError::ResponseError(_) => {
                    ApiError::internal_server_error(INTERNAL_SERVER_ERROR)
                }
                SdkError::ServiceError(_) => ApiError::internal_server_error(INTERNAL_SERVER_ERROR),
                _ => ApiError::internal_server_error(INTERNAL_SERVER_ERROR),
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::api::error::{ApiError, ApiErrorSource, ApiErrorSourceType};
    use crate::api::error_code::*;
    use serde_json::{Value, json};

    #[rstest::rstest]
    #[case::bad_request(ApiError::bad_request(BAD_REQUEST), json!({ "status": 400, "error": "BAD_REQUEST" }))]
    #[case::bad_request_msg(ApiError::bad_request(BAD_REQUEST).with_message("foo"), json!({ "status": 400, "error": "BAD_REQUEST", "message": "foo" }))]
    #[case::unauthorized(ApiError::unauthorized(UNAUTHORIZED), json!({ "status": 401, "error": "UNAUTHORIZED" }))]
    #[case::forbidden(ApiError::forbidden(FORBIDDEN), json!({ "status": 403, "error": "FORBIDDEN" }))]
    #[case::not_found(ApiError::not_found(NOT_FOUND), json!({ "status": 404, "error": "NOT_FOUND" }))]
    #[case::conflict(ApiError::conflict(CONFLICT), json!({ "status": 409, "error": "CONFLICT" }))]
    #[case::unprocessable_entity(ApiError::unprocessable_entity(UNPROCESSABLE_ENTITY), json!({ "status": 422, "error": "UNPROCESSABLE_ENTITY" }))]
    #[case::internal_server_error(ApiError::internal_server_error(INTERNAL_SERVER_ERROR), json!({ "status": 500, "error": "INTERNAL_SERVER_ERROR" }))]
    #[case::service_unavailable(ApiError::service_unavailable(SERVICE_UNAVAILABLE), json!({ "status": 503, "error": "SERVICE_UNAVAILABLE" }))]
    #[case::gateway_timeout(ApiError::gateway_time_out(GATEWAY_TIMEOUT), json!({ "status": 504, "error": "GATEWAY_TIMEOUT" }))]
    fn should_serialize_api_error(#[case] error: ApiError, #[case] expected: Value) {
        let actual = serde_json::to_value(error).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn should_serialize_api_error_with_query_field() {
        let error = ApiError::bad_request(BAD_REQUEST).with_query_field("limit");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 400,
                "error": "BAD_REQUEST",
                "source": {
                    "field": "limit",
                    "type": "QUERY"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_header_field() {
        let error = ApiError::unauthorized(UNAUTHORIZED).with_header_field("Authorization");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 401,
                "error": "UNAUTHORIZED",
                "source": {
                    "field": "Authorization",
                    "type": "HEADER"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_path_field() {
        let error = ApiError::not_found(NOT_FOUND).with_path_field("user_id");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 404,
                "error": "NOT_FOUND",
                "source": {
                    "field": "user_id",
                    "type": "PATH"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_body_field() {
        let error = ApiError::unprocessable_entity(UNPROCESSABLE_ENTITY).with_body_field("email");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 422,
                "error": "UNPROCESSABLE_ENTITY",
                "source": {
                    "field": "email",
                    "type": "BODY"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_source_struct() {
        let source = ApiErrorSource {
            field: "x-custom-header",
            source_type: ApiErrorSourceType::Header,
        };
        let error = ApiError::bad_request(BAD_REQUEST).with_source(source);
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 400,
                "error": "BAD_REQUEST",
                "source": {
                    "field": "x-custom-header",
                    "type": "HEADER"
                }
            })
        );
    }

    #[test]
    fn should_serialize_api_error_with_message_and_source() {
        let error = ApiError::bad_request(BAD_REQUEST)
            .with_message("Invalid format")
            .with_body_field("username");
        let json = serde_json::to_value(error).unwrap();
        assert_eq!(
            json,
            json!({
                "status": 400,
                "error": "BAD_REQUEST",
                "message": "Invalid format",
                "source": {
                    "field": "username",
                    "type": "BODY"
                }
            })
        );
    }
}

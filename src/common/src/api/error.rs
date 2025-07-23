use crate::api::api_gateway_proxy_response_builder::ApiGatewayProxyResponseBuilder;
use crate::api::error_code::ApiErrorCode;
use aws_lambda_events::apigw::ApiGatewayProxyResponse;
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
            source_type: ApiErrorSourceType::HEADER,
        });
        self
    }

    pub fn with_query_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::QUERY,
        });
        self
    }

    pub fn with_path_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::PATH,
        });
        self
    }
    pub fn with_body_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::BODY,
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

impl From<ApiError> for ApiGatewayProxyResponse {
    fn from(api_error: ApiError) -> Self {
        ApiGatewayProxyResponseBuilder::json(api_error.status.into())
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
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApiErrorSourceType {
    HEADER,
    PATH,
    QUERY,
    BODY,
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
    use crate::api::error::ApiError;
    use crate::api::error_code::*;
    use serde_json::{Value, json};

    #[rstest::rstest]
    #[case::bad_request(ApiError::bad_request(BAD_REQUEST), json!({ "status": 400, "error": "BAD_REQUEST" })
    )]
    #[case::bad_request_msg(ApiError::bad_request(BAD_REQUEST).with_message("foo"), json!({ "status": 400, "error": "BAD_REQUEST", "message": "foo" })
    )]
    fn should_serialize_api_error(#[case] error: ApiError, #[case] expected: Value) {
        let actual = serde_json::to_value(error).unwrap();
        assert_eq!(expected, actual);
    }
}

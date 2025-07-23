use serde::Serialize;
use std::fmt::Display;

#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct ApiErrorCode(&'static str);

pub const INTERNAL_SERVER_ERROR: ApiErrorCode = ApiErrorCode("INTERNAL_SERVER_ERROR");
pub const BAD_REQUEST: ApiErrorCode = ApiErrorCode("BAD_REQUEST");
pub const GATEWAY_TIMEOUT: ApiErrorCode = ApiErrorCode("GATEWAY_TIMEOUT");

// region impl ApiErrorCode

impl Display for ApiErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ApiErrorCode {
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl From<ApiErrorCode> for &'static str {
    fn from(api_error: ApiErrorCode) -> Self {
        api_error.0
    }
}

impl From<ApiErrorCode> for String {
    fn from(api_error: ApiErrorCode) -> Self {
        api_error.0.to_owned()
    }
}

// endregion

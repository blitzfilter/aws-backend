use serde::Serialize;
use std::fmt::Display;

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(transparent)]
pub struct ApiErrorCode(&'static str);

pub const BAD_REQUEST: ApiErrorCode = ApiErrorCode("BAD_REQUEST");
pub const UNAUTHORIZED: ApiErrorCode = ApiErrorCode("UNAUTHORIZED");
pub const FORBIDDEN: ApiErrorCode = ApiErrorCode("FORBIDDEN");
pub const NOT_FOUND: ApiErrorCode = ApiErrorCode("NOT_FOUND");
pub const CONFLICT: ApiErrorCode = ApiErrorCode("CONFLICT");
pub const UNPROCESSABLE_ENTITY: ApiErrorCode = ApiErrorCode("UNPROCESSABLE_ENTITY");
pub const INTERNAL_SERVER_ERROR: ApiErrorCode = ApiErrorCode("INTERNAL_SERVER_ERROR");
pub const SERVICE_UNAVAILABLE: ApiErrorCode = ApiErrorCode("SERVICE_UNAVAILABLE");
pub const GATEWAY_TIMEOUT: ApiErrorCode = ApiErrorCode("GATEWAY_TIMEOUT");

pub const BAD_QUERY_PARAMETER_VALUE: ApiErrorCode = ApiErrorCode("BAD_QUERY_PARAMETER_VALUE");
pub const BAD_HEADER_VALUE: ApiErrorCode = ApiErrorCode("BAD_HEADER_VALUE");
pub const BAD_PARAMETER: ApiErrorCode = ApiErrorCode("BAD_PARAMETER_VALUE");

pub const BAD_PAGE_FROM_VALUE: ApiErrorCode = ApiErrorCode("BAD_PAGE_FROM_VALUE");
pub const BAD_PAGE_SIZE_VALUE: ApiErrorCode = ApiErrorCode("BAD_PAGE_SIZE_VALUE");
pub const BAD_SORT_VALUE: ApiErrorCode = ApiErrorCode("BAD_SORT_VALUE");
pub const BAD_ORDER_VALUE: ApiErrorCode = ApiErrorCode("BAD_ORDER_VALUE");

pub const ITEM_NOT_FOUND: ApiErrorCode = ApiErrorCode("ITEM_NOT_FOUND");
pub const MONETARY_AMOUNT_OVERFLOW: ApiErrorCode = ApiErrorCode("MONETARY_AMOUNT_OVERFLOW");
pub const TEXT_QUERY_TOO_SHORT: ApiErrorCode = ApiErrorCode("TEXT_QUERY_TOO_SHORT");

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

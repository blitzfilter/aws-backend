use crate::auth::AuthError;
use admin_overview_service::GetAdminOverviewError;
use axum::Json;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use billing_service::use_cases::{
    CreateBillingCheckoutSessionError, CreateBillingManagementSessionError,
    CreateBillingPortalSessionError,
};
use listing_source_service::use_cases::commands::create_listing_source::CreateListingSourceError;
use listing_source_service::use_cases::commands::update_listing_source::UpdateListingSourceError;
use listing_source_service::use_cases::queries::get_listing_source::GetListingSourceError;
use listing_source_service::use_cases::queries::search_listing_sources::SearchListingSourcesError;
use notification_service::use_cases::commands::delete_notification::DeleteNotificationError;
use notification_service::use_cases::commands::delete_notifications::DeleteNotificationsError;
use notification_service::use_cases::commands::update_all_notifications_seen::UpdateAllNotificationsSeenError;
use notification_service::use_cases::commands::update_notification_seen::UpdateNotificationSeenError;
use notification_service::use_cases::commands::update_notifications_seen::UpdateNotificationsSeenError;
use notification_service::use_cases::queries::list_notifications::ListNotificationsError;
use oauth_service::error::OAuthServiceError;
use partnership_service::use_cases::queries::list_administered_listing_sources::ListAdministeredListingSourcesError;
use partnership_service::use_cases::{
    commands::{
        approve_partnership_application::ApprovePartnershipApplicationError,
        dissolve_partnership::DissolvePartnershipError,
        grant_partnership_listing_source::GrantPartnershipListingSourceError,
        grant_partnership_membership::GrantPartnershipMembershipError,
        mark_partnership_application_in_review::MarkPartnershipApplicationInReviewError,
        reject_partnership_application::RejectPartnershipApplicationError,
        revoke_partnership_listing_source::RevokePartnershipListingSourceError,
        revoke_partnership_membership::RevokePartnershipMembershipError,
        submit_partnership_application::SubmitPartnershipApplicationError,
        withdraw_partnership_application::WithdrawPartnershipApplicationError,
    },
    queries::{
        get_admin_partnership::GetAdminPartnershipError,
        get_own_partnership_application::GetOwnPartnershipApplicationError,
        get_partnership_application::GetPartnershipApplicationError,
        list_admin_partnership_applications::ListAdminPartnershipApplicationsError,
        list_admin_partnerships::ListAdminPartnershipsError,
        list_own_partnership_applications::ListOwnPartnershipApplicationsError,
    },
};
use party_service::use_cases::commands::create_party::CreatePartyError;
use party_service::use_cases::commands::update_party::UpdatePartyError;
use party_service::use_cases::queries::get_party::GetPartyError;
use party_service::use_cases::queries::search_parties::SearchPartiesError;
use product_listing_service::use_cases::{
    CreateProductListingError, GetProductListingError, GetProductListingHistoryError,
    GetSimilarProductListingsError, IngestWoocommerceProductListingError,
    SearchProductListingsError, UpdateProductListingError, UpsertProductListingError,
    WithdrawProductListingError,
};
use search_filter_service::use_cases::{
    CreateSearchFilterError, DeleteOwnedSearchFilterError, GetOwnedSearchFilterError,
    ListOwnedSearchFiltersError, ListSearchFilterMatchesError, UpdateOwnedSearchFilterError,
    UpdateSearchFilterMatchFeedbackError,
};
use serde::Serialize;

use std::error::Error;
use std::fmt::{Display, Formatter};
use user_service::use_cases::commands::change_user_role::ChangeUserRoleError;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierError;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenError;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenError;
use user_service::use_cases::commands::delete_access_tokens::DeleteAccessTokensError;
use user_service::use_cases::commands::delete_user::DeleteUserError;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenError;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileError;
use user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionError;
use user_service::use_cases::queries::admin_get_user::AdminGetUserError;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminError;
use user_service::use_cases::queries::get_access_token::GetAccessTokenError;
use user_service::use_cases::queries::get_own_user::GetOwnUserError;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensError;
use user_service::use_cases::queries::list_admin_access_tokens::ListAdminAccessTokensError;
use user_service::use_cases::queries::search_users::SearchUsersError;
use watchlist_service::use_cases::{
    ListWatchlistError, UnwatchProductListingError, UpdateWatchlistProductListingError,
    WatchProductListingError,
};

#[derive(Debug, Serialize)]
pub(crate) struct ApiError {
    status: u16,
    title: &'static str,
    error: ApiErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<ApiErrorSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip)]
    cause: Option<Box<dyn Error + Send + Sync>>,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(transparent)]
pub(crate) struct ApiErrorCode(&'static str);

pub(crate) const ADMIN_OVERVIEW_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("ADMIN_OVERVIEW_INTERNAL_ERROR");
pub(crate) const ADMIN_OVERVIEW_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("ADMIN_OVERVIEW_TEMPORARILY_UNAVAILABLE");
pub(crate) const AUTH_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("AUTH_INTERNAL_ERROR");
pub(crate) const AUTH_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("AUTH_TEMPORARILY_UNAVAILABLE");
pub(crate) const ACCESS_TOKEN_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("ACCESS_TOKEN_INTERNAL_ERROR");
pub(crate) const ACCESS_TOKEN_NOT_FOUND: ApiErrorCode = ApiErrorCode("ACCESS_TOKEN_NOT_FOUND");
pub(crate) const ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE");
pub(crate) const INVALID_CREDENTIALS: ApiErrorCode = ApiErrorCode("INVALID_CREDENTIALS");
pub(crate) const BAD_BODY_VALUE: ApiErrorCode = ApiErrorCode("BAD_BODY_VALUE");
pub(crate) const BAD_HEADER_VALUE: ApiErrorCode = ApiErrorCode("BAD_HEADER_VALUE");
pub(crate) const BILLING_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("BILLING_INTERNAL_ERROR");
pub(crate) const BILLING_PROVIDER_FAILURE: ApiErrorCode = ApiErrorCode("BILLING_PROVIDER_FAILURE");
pub(crate) const BILLING_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("BILLING_TEMPORARILY_UNAVAILABLE");
pub(crate) const STRIPE_CUSTOMER_ALREADY_EXISTS: ApiErrorCode =
    ApiErrorCode("STRIPE_CUSTOMER_ALREADY_EXISTS");
pub(crate) const STRIPE_CUSTOMER_ASSOCIATION_CONFLICT: ApiErrorCode =
    ApiErrorCode("STRIPE_CUSTOMER_ASSOCIATION_CONFLICT");
pub(crate) const STRIPE_CUSTOMER_DOES_NOT_EXIST: ApiErrorCode =
    ApiErrorCode("STRIPE_CUSTOMER_DOES_NOT_EXIST");
pub(crate) const BAD_ORDER_VALUE: ApiErrorCode = ApiErrorCode("BAD_ORDER_VALUE");
pub(crate) const BAD_PATH_PARAMETER_VALUE: ApiErrorCode = ApiErrorCode("BAD_PATH_PARAMETER_VALUE");
pub(crate) const BAD_QUERY_PARAMETER_VALUE: ApiErrorCode =
    ApiErrorCode("BAD_QUERY_PARAMETER_VALUE");
pub(crate) const BAD_SORT_VALUE: ApiErrorCode = ApiErrorCode("BAD_SORT_VALUE");
pub(crate) const CONFLICT: ApiErrorCode = ApiErrorCode("CONFLICT");
pub(crate) const FORBIDDEN: ApiErrorCode = ApiErrorCode("FORBIDDEN");
pub(crate) const INVALID_UUID: ApiErrorCode = ApiErrorCode("INVALID_UUID");
pub(crate) const LISTING_SOURCE_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("LISTING_SOURCE_INTERNAL_ERROR");
pub(crate) const LISTING_SOURCE_NOT_FOUND: ApiErrorCode = ApiErrorCode("LISTING_SOURCE_NOT_FOUND");
pub(crate) const LISTING_SOURCE_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("LISTING_SOURCE_TEMPORARILY_UNAVAILABLE");
pub(crate) const PARTY_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("PARTY_INTERNAL_ERROR");
pub(crate) const PARTY_NOT_FOUND: ApiErrorCode = ApiErrorCode("PARTY_NOT_FOUND");
pub(crate) const PARTY_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("PARTY_TEMPORARILY_UNAVAILABLE");
pub(crate) const PRODUCT_LISTING_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("PRODUCT_LISTING_INTERNAL_ERROR");
pub(crate) const PRODUCT_LISTING_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("PRODUCT_LISTING_NOT_FOUND");
pub(crate) const PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE");
pub(crate) const SEARCH_FILTER_ALREADY_EXISTS: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_ALREADY_EXISTS");
pub(crate) const SEARCH_FILTER_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_INTERNAL_ERROR");
pub(crate) const SEARCH_FILTER_INVALID_PATCH: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_INVALID_PATCH");
pub(crate) const SEARCH_FILTER_MATCH_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_MATCH_NOT_FOUND");
pub(crate) const SEARCH_FILTER_NOT_FOUND: ApiErrorCode = ApiErrorCode("SEARCH_FILTER_NOT_FOUND");
pub(crate) const SEARCH_FILTER_QUOTA_EXCEEDED: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_QUOTA_EXCEEDED");
pub(crate) const SEARCH_FILTER_RESTRICTED_FEATURE: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_RESTRICTED_FEATURE");
pub(crate) const SEARCH_FILTER_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("SEARCH_FILTER_TEMPORARILY_UNAVAILABLE");
pub(crate) const INVALID_EMAIL: ApiErrorCode = ApiErrorCode("INVALID_EMAIL");
pub(crate) const NEWSLETTER_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("NEWSLETTER_INTERNAL_ERROR");
pub(crate) const NEWSLETTER_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("NEWSLETTER_TEMPORARILY_UNAVAILABLE");
pub(crate) const NOTIFICATION_NOT_FOUND: ApiErrorCode = ApiErrorCode("NOTIFICATION_NOT_FOUND");
pub(crate) const NOTIFICATION_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("NOTIFICATION_TEMPORARILY_UNAVAILABLE");
pub(crate) const USER_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("USER_INTERNAL_ERROR");
pub(crate) const USER_NOT_FOUND: ApiErrorCode = ApiErrorCode("USER_NOT_FOUND");
pub(crate) const USER_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("USER_TEMPORARILY_UNAVAILABLE");
pub(crate) const WATCHLIST_ENTRY_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("WATCHLIST_ENTRY_NOT_FOUND");
pub(crate) const WATCHLIST_QUOTA_EXCEEDED: ApiErrorCode = ApiErrorCode("WATCHLIST_QUOTA_EXCEEDED");
pub(crate) const WATCHLIST_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("WATCHLIST_INTERNAL_ERROR");
pub(crate) const WATCHLIST_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("WATCHLIST_TEMPORARILY_UNAVAILABLE");
pub(crate) const PARTNERSHIP_APPLICATION_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("PARTNERSHIP_APPLICATION_NOT_FOUND");
pub(crate) const PARTNERSHIP_APPLICATION_LISTING_SOURCE_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("PARTNERSHIP_APPLICATION_LISTING_SOURCE_NOT_FOUND");
pub(crate) const PARTNERSHIP_APPLICATION_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("PARTNERSHIP_APPLICATION_INTERNAL_ERROR");
pub(crate) const PARTNERSHIP_APPLICATION_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("PARTNERSHIP_APPLICATION_TEMPORARILY_UNAVAILABLE");
pub(crate) const PARTNERSHIP_INTERNAL_ERROR: ApiErrorCode =
    ApiErrorCode("PARTNERSHIP_INTERNAL_ERROR");
pub(crate) const PARTNERSHIP_NOT_FOUND: ApiErrorCode = ApiErrorCode("PARTNERSHIP_NOT_FOUND");
pub(crate) const PARTNERSHIP_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("PARTNERSHIP_TEMPORARILY_UNAVAILABLE");
pub(crate) const OAUTH_AUTHORIZATION_CODE_EXPIRED: ApiErrorCode =
    ApiErrorCode("OAUTH_AUTHORIZATION_CODE_EXPIRED");
pub(crate) const OAUTH_AUTHORIZATION_CODE_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("OAUTH_AUTHORIZATION_CODE_NOT_FOUND");
pub(crate) const OAUTH_CLIENT_NOT_FOUND: ApiErrorCode = ApiErrorCode("OAUTH_CLIENT_NOT_FOUND");
pub(crate) const OAUTH_INTERNAL_ERROR: ApiErrorCode = ApiErrorCode("OAUTH_INTERNAL_ERROR");
pub(crate) const OAUTH_INVALID_CLIENT_METADATA: ApiErrorCode =
    ApiErrorCode("OAUTH_INVALID_CLIENT_METADATA");
pub(crate) const OAUTH_INVALID_CLIENT_SECRET: ApiErrorCode =
    ApiErrorCode("OAUTH_INVALID_CLIENT_SECRET");
pub(crate) const OAUTH_INVALID_CODE_VERIFIER: ApiErrorCode =
    ApiErrorCode("OAUTH_INVALID_CODE_VERIFIER");
pub(crate) const OAUTH_INVALID_REDIRECT_URI: ApiErrorCode =
    ApiErrorCode("OAUTH_INVALID_REDIRECT_URI");
pub(crate) const OAUTH_INVALID_SCOPE: ApiErrorCode = ApiErrorCode("OAUTH_INVALID_SCOPE");
pub(crate) const OAUTH_TEMPORARILY_UNAVAILABLE: ApiErrorCode =
    ApiErrorCode("OAUTH_TEMPORARILY_UNAVAILABLE");
pub(crate) const OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND: ApiErrorCode =
    ApiErrorCode("OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND");

impl Display for ApiErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
pub(crate) struct ApiErrorSource {
    field: &'static str,
    #[serde(rename = "type")]
    source_type: ApiErrorSourceType,
}

#[derive(Debug, Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "UPPERCASE")]
enum ApiErrorSourceType {
    Header,
    Path,
    Query,
}

impl ApiError {
    pub(crate) fn code(&self) -> ApiErrorCode {
        self.error
    }

    pub(crate) fn new(status: StatusCode, title: &'static str, error: ApiErrorCode) -> Self {
        Self {
            status: status.as_u16(),
            title,
            error,
            source: None,
            detail: None,
            cause: None,
        }
    }

    pub(crate) fn bad_request(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "Bad Request", error)
    }

    pub(crate) fn unauthorized(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "Unauthorized", error)
    }

    pub(crate) fn forbidden(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::FORBIDDEN, "Forbidden", error)
    }

    pub(crate) fn not_found(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::NOT_FOUND, "Not Found", error)
    }

    pub(crate) fn conflict(error: ApiErrorCode) -> Self {
        Self::new(StatusCode::CONFLICT, "Conflict", error)
    }

    pub(crate) fn unprocessable_content(error: ApiErrorCode) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Unprocessable Content",
            error,
        )
    }

    pub(crate) fn internal_server_error(error: ApiErrorCode) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal Server Error",
            error,
        )
    }

    pub(crate) fn service_unavailable(error: ApiErrorCode) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Service Unavailable",
            error,
        )
    }

    pub(crate) fn with_header_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Header,
        });
        self
    }

    pub(crate) fn with_query_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Query,
        });
        self
    }

    pub(crate) fn with_path_field(mut self, field: &'static str) -> Self {
        self.source = Some(ApiErrorSource {
            field,
            source_type: ApiErrorSourceType::Path,
        });
        self
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn status_code(&self) -> StatusCode {
        match StatusCode::from_u16(self.status) {
            Ok(status) => status,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} - {}", self.status, self.error)?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        if let Some(cause) = &self.cause {
            write!(f, ": {cause}")?;
        }
        Ok(())
    }
}

impl Error for ApiError {}

impl From<AuthError> for ApiError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::TemporarilyUnavailable => {
                ApiError::service_unavailable(AUTH_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Authentication is temporarily unavailable.")
            }
            AuthError::Internal(_) => ApiError::internal_server_error(AUTH_INTERNAL_ERROR)
                .with_detail("Authentication failed internally."),
            AuthError::MissingCredentials
            | AuthError::InvalidAuthorizationHeader
            | AuthError::MalformedCredentials
            | AuthError::InvalidCredentials
            | AuthError::MissingClaim(_)
            | AuthError::InvalidClaimType(_)
            | AuthError::JwksKeyNotFound
            | AuthError::JwksFetch(_) => ApiError::unauthorized(INVALID_CREDENTIALS)
                .with_header_field("Authorization")
                .with_detail("Bearer token is invalid."),
        }
    }
}

impl From<GetAdminOverviewError> for ApiError {
    fn from(error: GetAdminOverviewError) -> Self {
        match error {
            GetAdminOverviewError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetAdminOverviewError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetAdminOverviewError::AdminAuthorizationTemporarilyUnavailable { .. }
            | GetAdminOverviewError::ReaderTemporarilyUnavailable { .. }
            | GetAdminOverviewError::BeginTransaction { .. }
            | GetAdminOverviewError::CommitTransaction { .. } => {
                ApiError::service_unavailable(ADMIN_OVERVIEW_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Admin overview is temporarily unavailable.")
            }
            GetAdminOverviewError::AdminAuthorizationInvalidReadModel { .. }
            | GetAdminOverviewError::AdminAuthorizationInternal { .. }
            | GetAdminOverviewError::ReaderInvalidReadModel { .. }
            | GetAdminOverviewError::ReaderInternal { .. } => {
                ApiError::internal_server_error(ADMIN_OVERVIEW_INTERNAL_ERROR)
                    .with_detail("Admin overview failed internally.")
            }
        }
    }
}

impl From<ListNotificationsError> for ApiError {
    fn from(error: ListNotificationsError) -> Self {
        match error {
            ListNotificationsError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListNotificationsError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListNotificationsError::ReadFailed(_) => {
                ApiError::service_unavailable(NOTIFICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Notifications are temporarily unavailable.")
            }
        }
    }
}

impl From<UpdateNotificationSeenError> for ApiError {
    fn from(error: UpdateNotificationSeenError) -> Self {
        match error {
            UpdateNotificationSeenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateNotificationSeenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateNotificationSeenError::NotFound => ApiError::not_found(NOTIFICATION_NOT_FOUND)
                .with_detail("Notification was not found."),
            UpdateNotificationSeenError::UpdateFailed(_) => {
                ApiError::service_unavailable(NOTIFICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Notifications are temporarily unavailable.")
            }
        }
    }
}

impl From<UpdateNotificationsSeenError> for ApiError {
    fn from(error: UpdateNotificationsSeenError) -> Self {
        match error {
            UpdateNotificationsSeenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateNotificationsSeenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateNotificationsSeenError::EmptyNotificationIds => {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("notificationIds must contain at least one notification UUID.")
            }
            UpdateNotificationsSeenError::UpdateFailed(_) => {
                ApiError::service_unavailable(NOTIFICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Notifications are temporarily unavailable.")
            }
        }
    }
}

impl From<UpdateAllNotificationsSeenError> for ApiError {
    fn from(error: UpdateAllNotificationsSeenError) -> Self {
        match error {
            UpdateAllNotificationsSeenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateAllNotificationsSeenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateAllNotificationsSeenError::UpdateFailed(_) => {
                ApiError::service_unavailable(NOTIFICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Notifications are temporarily unavailable.")
            }
        }
    }
}

impl From<DeleteNotificationError> for ApiError {
    fn from(error: DeleteNotificationError) -> Self {
        match error {
            DeleteNotificationError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteNotificationError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteNotificationError::NotFound => ApiError::not_found(NOTIFICATION_NOT_FOUND)
                .with_detail("Notification was not found."),
            DeleteNotificationError::DeleteFailed(_) => {
                ApiError::service_unavailable(NOTIFICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Notifications are temporarily unavailable.")
            }
        }
    }
}

impl From<DeleteNotificationsError> for ApiError {
    fn from(error: DeleteNotificationsError) -> Self {
        match error {
            DeleteNotificationsError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteNotificationsError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteNotificationsError::DeleteFailed(_) => {
                ApiError::service_unavailable(NOTIFICATION_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Notifications are temporarily unavailable.")
            }
        }
    }
}

impl From<ListOwnedSearchFiltersError> for ApiError {
    fn from(error: ListOwnedSearchFiltersError) -> Self {
        match error {
            ListOwnedSearchFiltersError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListOwnedSearchFiltersError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListOwnedSearchFiltersError::SearchFilterListReadFailed { .. } => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filters are temporarily unavailable.")
            }
        }
    }
}

impl From<GetOwnedSearchFilterError> for ApiError {
    fn from(error: GetOwnedSearchFilterError) -> Self {
        match error {
            GetOwnedSearchFilterError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetOwnedSearchFilterError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetOwnedSearchFilterError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            GetOwnedSearchFilterError::SearchFilterReadFailed { .. } => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter is temporarily unavailable.")
            }
        }
    }
}

impl From<CreateSearchFilterError> for ApiError {
    fn from(error: CreateSearchFilterError) -> Self {
        match error {
            CreateSearchFilterError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateSearchFilterError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateSearchFilterError::SearchFilterAlreadyExists => {
                ApiError::conflict(SEARCH_FILTER_ALREADY_EXISTS)
                    .with_detail("Search filter already exists.")
            }
            CreateSearchFilterError::UserNotFound => ApiError::not_found(USER_NOT_FOUND)
                .with_detail("User was not found."),
            CreateSearchFilterError::SearchFilterQuotaExceeded {
                active_count,
                quota,
            } => ApiError::unprocessable_content(SEARCH_FILTER_QUOTA_EXCEEDED).with_detail(
                format!(
                    "Exceeded the maximum amount of search filters. There are already {active_count}/{quota} active search filters occupied."
                ),
            ),
            CreateSearchFilterError::SearchFilterFeatureRestricted { feature } => {
                ApiError::unprocessable_content(SEARCH_FILTER_RESTRICTED_FEATURE).with_detail(
                    format!(
                        "Search filter contains forbidden search field '{feature}' which requires a higher user tier."
                    ),
                )
            }
            CreateSearchFilterError::PersistedSearchFilterStateInvalid { .. } => {
                ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                    .with_detail("Search filter state is invalid.")
            }
            CreateSearchFilterError::EmbeddingGenerationFailed { .. }
            | CreateSearchFilterError::UserTierEntitlementsLockFailed { .. }
            | CreateSearchFilterError::SearchFilterQuotaReadFailed { .. }
            | CreateSearchFilterError::SearchFilterInsertFailed { .. }
            | CreateSearchFilterError::BeginTransactionFailed
            | CreateSearchFilterError::CommitTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter could not be created right now.")
            }
        }
    }
}

impl From<UpdateOwnedSearchFilterError> for ApiError {
    fn from(error: UpdateOwnedSearchFilterError) -> Self {
        match error {
            UpdateOwnedSearchFilterError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateOwnedSearchFilterError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateOwnedSearchFilterError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            UpdateOwnedSearchFilterError::UserNotFound => ApiError::not_found(USER_NOT_FOUND)
                .with_detail("User was not found."),
            UpdateOwnedSearchFilterError::SearchFilterQuotaExceeded {
                active_count,
                quota,
            } => ApiError::unprocessable_content(SEARCH_FILTER_QUOTA_EXCEEDED).with_detail(
                format!(
                    "Exceeded the maximum amount of search filters. There are already {active_count}/{quota} active search filters occupied."
                ),
            ),
            UpdateOwnedSearchFilterError::SearchFilterFeatureRestricted { feature } => {
                ApiError::unprocessable_content(SEARCH_FILTER_RESTRICTED_FEATURE).with_detail(
                    format!(
                        "Search filter contains forbidden search field '{feature}' which requires a higher user tier."
                    ),
                )
            }
            UpdateOwnedSearchFilterError::InvalidSearchFilterPatch => {
                ApiError::bad_request(SEARCH_FILTER_INVALID_PATCH)
                    .with_detail("Search filter patch is invalid.")
            }
            UpdateOwnedSearchFilterError::SearchFilterConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Search filter was changed concurrently.")
            }
            UpdateOwnedSearchFilterError::PersistedSearchFilterStateInvalid { .. } => {
                ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                    .with_detail("Search filter state is invalid.")
            }
            UpdateOwnedSearchFilterError::EmbeddingGenerationFailed { .. }
            | UpdateOwnedSearchFilterError::UserTierEntitlementsLockFailed { .. }
            | UpdateOwnedSearchFilterError::SearchFilterQuotaReadFailed { .. }
            | UpdateOwnedSearchFilterError::SearchFilterLookupFailed { .. }
            | UpdateOwnedSearchFilterError::SearchFilterUpdateFailed { .. }
            | UpdateOwnedSearchFilterError::BeginTransactionFailed
            | UpdateOwnedSearchFilterError::CommitTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter could not be updated right now.")
            }
        }
    }
}

impl From<DeleteOwnedSearchFilterError> for ApiError {
    fn from(error: DeleteOwnedSearchFilterError) -> Self {
        match error {
            DeleteOwnedSearchFilterError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteOwnedSearchFilterError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteOwnedSearchFilterError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            DeleteOwnedSearchFilterError::PersistedSearchFilterStateInvalid { .. } => {
                ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                    .with_detail("Search filter state is invalid.")
            }
            DeleteOwnedSearchFilterError::SearchFilterLookupFailed { .. }
            | DeleteOwnedSearchFilterError::SearchFilterDeletionFailed { .. }
            | DeleteOwnedSearchFilterError::BeginTransactionFailed
            | DeleteOwnedSearchFilterError::CommitTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter could not be deleted right now.")
            }
        }
    }
}

impl From<ListSearchFilterMatchesError> for ApiError {
    fn from(error: ListSearchFilterMatchesError) -> Self {
        match error {
            ListSearchFilterMatchesError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListSearchFilterMatchesError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListSearchFilterMatchesError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            ListSearchFilterMatchesError::ProductListingDetailsInvalid { .. }
            | ListSearchFilterMatchesError::MatchedProductListingMissing { .. }
            | ListSearchFilterMatchesError::PricingFxSnapshotInvalid { .. }
            | ListSearchFilterMatchesError::SaleFxSnapshotMismatch { .. }
            | ListSearchFilterMatchesError::ProductListingPriceConversionFailed { .. }
            | ListSearchFilterMatchesError::HiddenProductListingRedactionFailed { .. } => {
                ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                    .with_detail("Search filter match product data is invalid.")
            }
            ListSearchFilterMatchesError::SearchFilterMatchReadFailed { .. }
            | ListSearchFilterMatchesError::ProductListingDetailsReadFailed { .. }
            | ListSearchFilterMatchesError::CurrentPricingFxSnapshotMissing
            | ListSearchFilterMatchesError::SalePricingFxSnapshotMissing { .. }
            | ListSearchFilterMatchesError::PricingFxSnapshotUnavailable { .. }
            | ListSearchFilterMatchesError::BeginPricingTransactionFailed
            | ListSearchFilterMatchesError::CommitPricingTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter matches are temporarily unavailable.")
            }
        }
    }
}

impl From<UpdateSearchFilterMatchFeedbackError> for ApiError {
    fn from(error: UpdateSearchFilterMatchFeedbackError) -> Self {
        match error {
            UpdateSearchFilterMatchFeedbackError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateSearchFilterMatchFeedbackError::ActorMayNotManageSearchFilter => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateSearchFilterMatchFeedbackError::SearchFilterNotFound => {
                ApiError::not_found(SEARCH_FILTER_NOT_FOUND)
                    .with_detail("Search filter was not found.")
            }
            UpdateSearchFilterMatchFeedbackError::SearchFilterMatchNotFound => {
                ApiError::not_found(SEARCH_FILTER_MATCH_NOT_FOUND)
                    .with_detail("Search filter match was not found.")
            }
            UpdateSearchFilterMatchFeedbackError::PersistedSearchFilterStateInvalid { .. }
            | UpdateSearchFilterMatchFeedbackError::PersistedSearchFilterMatchStateInvalid {
                ..
            } => ApiError::internal_server_error(SEARCH_FILTER_INTERNAL_ERROR)
                .with_detail("Search filter state is invalid."),
            UpdateSearchFilterMatchFeedbackError::SearchFilterLookupFailed { .. }
            | UpdateSearchFilterMatchFeedbackError::SearchFilterMatchLookupFailed { .. }
            | UpdateSearchFilterMatchFeedbackError::SearchFilterMatchUpdateFailed { .. }
            | UpdateSearchFilterMatchFeedbackError::BeginTransactionFailed
            | UpdateSearchFilterMatchFeedbackError::CommitTransactionFailed => {
                ApiError::service_unavailable(SEARCH_FILTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Search filter match could not be updated right now.")
            }
        }
    }
}

impl From<CreateListingSourceError> for ApiError {
    fn from(error: CreateListingSourceError) -> Self {
        match error {
            CreateListingSourceError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateListingSourceError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateListingSourceError::OperatorPartyNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source operator was not found.")
            }
            CreateListingSourceError::ListingIngestionConfigurationMismatch => {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("Listing source ingestion configuration is invalid.")
            }
            CreateListingSourceError::OperatorPartySlugConflict { .. }
            | CreateListingSourceError::SlugConflict { .. }
            | CreateListingSourceError::ShopifyDomainConflict { .. } => {
                ApiError::conflict(CONFLICT)
                    .with_detail("Listing source conflicts with current state.")
            }
            CreateListingSourceError::TemporarilyUnavailable { .. }
            | CreateListingSourceError::BeginTransactionFailed
            | CreateListingSourceError::CommitTransactionFailed => {
                ApiError::service_unavailable(LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Listing source could not be created right now.")
            }
            CreateListingSourceError::InvalidPersistedState { .. }
            | CreateListingSourceError::Internal { .. } => {
                ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                    .with_detail("Listing source create failed internally.")
            }
        }
    }
}

impl From<UpdateListingSourceError> for ApiError {
    fn from(error: UpdateListingSourceError) -> Self {
        match error {
            UpdateListingSourceError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateListingSourceError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateListingSourceError::NotFound => ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                .with_detail("Listing source was not found."),
            UpdateListingSourceError::OperatorPartyNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source operator was not found.")
            }
            UpdateListingSourceError::ListingIngestionConfigurationMismatch => {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("Listing source ingestion configuration is invalid.")
            }
            UpdateListingSourceError::ConcurrencyConflict
            | UpdateListingSourceError::SlugConflict { .. }
            | UpdateListingSourceError::ShopifyDomainConflict { .. } => {
                ApiError::conflict(CONFLICT)
                    .with_detail("Listing source conflicts with current state.")
            }
            UpdateListingSourceError::TemporarilyUnavailable { .. }
            | UpdateListingSourceError::BeginTransactionFailed
            | UpdateListingSourceError::CommitTransactionFailed => {
                ApiError::service_unavailable(LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Listing source could not be updated right now.")
            }
            UpdateListingSourceError::InvalidPersistedState { .. }
            | UpdateListingSourceError::Internal { .. } => {
                ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                    .with_detail("Listing source update failed internally.")
            }
        }
    }
}

impl From<GetListingSourceError> for ApiError {
    fn from(error: GetListingSourceError) -> Self {
        match error {
            GetListingSourceError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetListingSourceError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetListingSourceError::NotFound => ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                .with_detail("Listing source was not found."),
            GetListingSourceError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Listing source details are temporarily unavailable.")
            }
            GetListingSourceError::InvalidReadModel { .. }
            | GetListingSourceError::Internal { .. } => {
                ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                    .with_detail("Listing source details failed internally.")
            }
        }
    }
}

impl From<SearchListingSourcesError> for ApiError {
    fn from(error: SearchListingSourcesError) -> Self {
        match error {
            SearchListingSourcesError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            SearchListingSourcesError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            SearchListingSourcesError::TemporarilyUnavailable { .. }
            | SearchListingSourcesError::BeginTransactionFailed
            | SearchListingSourcesError::CommitTransactionFailed => {
                ApiError::service_unavailable(LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Listing source search is temporarily unavailable.")
            }
            SearchListingSourcesError::InvalidReadModel { .. }
            | SearchListingSourcesError::Internal { .. } => {
                ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                    .with_detail("Listing source search failed internally.")
            }
        }
    }
}

impl From<ListAdministeredListingSourcesError> for ApiError {
    fn from(error: ListAdministeredListingSourcesError) -> Self {
        match error {
            ListAdministeredListingSourcesError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListAdministeredListingSourcesError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListAdministeredListingSourcesError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Administered listing sources are temporarily unavailable.")
            }
            ListAdministeredListingSourcesError::InvalidReadModel { .. }
            | ListAdministeredListingSourcesError::Internal { .. } => {
                ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                    .with_detail("Administered listing sources failed internally.")
            }
        }
    }
}

impl From<CreateProductListingError> for ApiError {
    fn from(error: CreateProductListingError) -> Self {
        match error {
            CreateProductListingError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateProductListingError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateProductListingError::ListingSourceNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            CreateProductListingError::SourceListingAlreadyExists => ApiError::conflict(CONFLICT)
                .with_detail("ProductListing conflicts with current state."),
            CreateProductListingError::ProductListingTitleSlugAlreadyExists
            | CreateProductListingError::ProductListingTitleSlugGenerationExhausted => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("ProductListing title slug generation is temporarily unavailable.")
            }
            CreateProductListingError::InvalidProductListing => {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("ProductListing create is invalid.")
            }
            CreateProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. }
            | CreateProductListingError::PersistenceFailed
            | CreateProductListingError::EventAppenderFailed { .. }
            | CreateProductListingError::BeginTransactionFailed
            | CreateProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("ProductListing create is temporarily unavailable.")
            }
            CreateProductListingError::PartnerAuthorizationInternal { .. }
            | CreateProductListingError::CreatedEventMissing => {
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                    .with_detail("ProductListing create failed internally.")
            }
        }
    }
}

impl From<UpdateProductListingError> for ApiError {
    fn from(error: UpdateProductListingError) -> Self {
        match error {
            UpdateProductListingError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateProductListingError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateProductListingError::ListingSourceNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            UpdateProductListingError::NotFound => ApiError::not_found(PRODUCT_LISTING_NOT_FOUND)
                .with_detail("ProductListing was not found."),
            UpdateProductListingError::ListingWithdrawn => {
                ApiError::conflict(CONFLICT).with_detail("ProductListing has been withdrawn.")
            }
            UpdateProductListingError::UrlRequired => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("ProductListing URL is required.")
            }
            UpdateProductListingError::InvalidProductListing => {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("ProductListing update is invalid.")
            }
            UpdateProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. }
            | UpdateProductListingError::PersistenceFailed
            | UpdateProductListingError::EventAppenderFailed { .. }
            | UpdateProductListingError::BeginTransactionFailed
            | UpdateProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("ProductListing update is temporarily unavailable.")
            }
            UpdateProductListingError::PartnerAuthorizationInternal { .. } => {
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                    .with_detail("ProductListing update failed internally.")
            }
        }
    }
}

impl From<WithdrawProductListingError> for ApiError {
    fn from(error: WithdrawProductListingError) -> Self {
        match error {
            WithdrawProductListingError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            WithdrawProductListingError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            WithdrawProductListingError::ListingSourceNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            WithdrawProductListingError::NotFound => ApiError::not_found(PRODUCT_LISTING_NOT_FOUND)
                .with_detail("ProductListing was not found."),
            WithdrawProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. }
            | WithdrawProductListingError::PersistenceFailed
            | WithdrawProductListingError::EventAppenderFailed { .. }
            | WithdrawProductListingError::BeginTransactionFailed
            | WithdrawProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("ProductListing withdrawal is temporarily unavailable.")
            }
            WithdrawProductListingError::PartnerAuthorizationInternal { .. } => {
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                    .with_detail("ProductListing withdrawal failed internally.")
            }
        }
    }
}

impl From<IngestWoocommerceProductListingError> for ApiError {
    fn from(error: IngestWoocommerceProductListingError) -> Self {
        match error {
            IngestWoocommerceProductListingError::MissingTitle
            | IngestWoocommerceProductListingError::MissingUrl
            | IngestWoocommerceProductListingError::InvalidPrice => {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("WooCommerce product payload is invalid.")
            }
            IngestWoocommerceProductListingError::MissingListingSourceCurrency
            | IngestWoocommerceProductListingError::MissingListingSourceLanguage => {
                ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                    .with_detail("WooCommerce listing source configuration is incomplete.")
            }
            IngestWoocommerceProductListingError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            IngestWoocommerceProductListingError::Forbidden
            | IngestWoocommerceProductListingError::ActorMayNotIngestForListingSource => {
                ApiError::forbidden(FORBIDDEN)
                    .with_detail("Actor is not a partner of this listing source.")
            }
            IngestWoocommerceProductListingError::ListingSourceNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            IngestWoocommerceProductListingError::WebhookSecretNotConfigured => {
                ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                    .with_detail("WooCommerce webhook secret is not configured.")
            }
            IngestWoocommerceProductListingError::InvalidSignature => {
                ApiError::unauthorized(BAD_HEADER_VALUE)
                    .with_header_field("x-wc-webhook-signature")
                    .with_detail("WooCommerce signature is invalid.")
            }
            IngestWoocommerceProductListingError::PartnerAuthorizationTemporarilyUnavailable {
                ..
            }
            | IngestWoocommerceProductListingError::ListingSourceTemporarilyUnavailable {
                ..
            } => ApiError::service_unavailable(LISTING_SOURCE_TEMPORARILY_UNAVAILABLE)
                .with_detail("WooCommerce webhook validation is temporarily unavailable."),
            IngestWoocommerceProductListingError::PartnerAuthorizationInternal { .. }
            | IngestWoocommerceProductListingError::InvalidListingSourceReadModel { .. } => {
                ApiError::internal_server_error(LISTING_SOURCE_INTERNAL_ERROR)
                    .with_detail("WooCommerce webhook validation failed internally.")
            }
            IngestWoocommerceProductListingError::InvalidProductListing { .. } => {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("WooCommerce product payload is invalid.")
            }
            IngestWoocommerceProductListingError::ListingWithdrawn => {
                ApiError::conflict(CONFLICT).with_detail("ProductListing has been withdrawn.")
            }
            IngestWoocommerceProductListingError::ProductListingTitleSlugGenerationExhausted
            | IngestWoocommerceProductListingError::ProductListingPersistenceFailed
            | IngestWoocommerceProductListingError::ProductListingEventAppenderFailed { .. }
            | IngestWoocommerceProductListingError::BeginTransactionFailed
            | IngestWoocommerceProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("WooCommerce product ingestion is temporarily unavailable.")
            }
        }
    }
}

impl From<UpsertProductListingError> for ApiError {
    fn from(error: UpsertProductListingError) -> Self {
        match error {
            UpsertProductListingError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpsertProductListingError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpsertProductListingError::ListingSourceNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            UpsertProductListingError::ListingWithdrawn => {
                ApiError::conflict(CONFLICT).with_detail("ProductListing has been withdrawn.")
            }
            UpsertProductListingError::InvalidProductListing { .. } => {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("ProductListing upsert is invalid.")
            }
            UpsertProductListingError::PartnerAuthorizationTemporarilyUnavailable { .. }
            | UpsertProductListingError::ProductListingTitleSlugGenerationExhausted
            | UpsertProductListingError::PersistenceFailed
            | UpsertProductListingError::EventAppenderFailed { .. }
            | UpsertProductListingError::BeginTransactionFailed
            | UpsertProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("ProductListing upsert is temporarily unavailable.")
            }
            UpsertProductListingError::PartnerAuthorizationInternal { .. } => {
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                    .with_detail("ProductListing upsert failed internally.")
            }
        }
    }
}

impl From<GetProductListingError> for ApiError {
    fn from(error: GetProductListingError) -> Self {
        match error {
            GetProductListingError::NotFound => ApiError::not_found(PRODUCT_LISTING_NOT_FOUND)
                .with_detail("ProductListing was not found."),
            GetProductListingError::ProductListingDetailsQueryFailed
            | GetProductListingError::PricingFxSnapshotMissing
            | GetProductListingError::PricingFxSnapshotUnavailable { .. }
            | GetProductListingError::BeginTransactionFailed
            | GetProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("ProductListing details are temporarily unavailable.")
            }
            GetProductListingError::ProductListingDetailsReadModelInvalid
            | GetProductListingError::PricingFxSnapshotInvalid { .. }
            | GetProductListingError::SaleObservationFxSnapshotMismatch { .. }
            | GetProductListingError::ProductListingPriceConversionFailed { .. } => {
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                    .with_detail("ProductListing details failed internally.")
            }
        }
    }
}

impl From<GetProductListingHistoryError> for ApiError {
    fn from(error: GetProductListingHistoryError) -> Self {
        match error {
            GetProductListingHistoryError::NotFound => {
                ApiError::not_found(PRODUCT_LISTING_NOT_FOUND)
                    .with_detail("ProductListing was not found.")
            }
            GetProductListingHistoryError::QueryFailed { .. }
            | GetProductListingHistoryError::BeginTransactionFailed
            | GetProductListingHistoryError::CommitTransactionFailed => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("ProductListing history is temporarily unavailable.")
            }
            GetProductListingHistoryError::InvalidReadModel { .. } => {
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                    .with_detail("ProductListing history contains invalid event data.")
            }
        }
    }
}

impl From<GetSimilarProductListingsError> for ApiError {
    fn from(error: GetSimilarProductListingsError) -> Self {
        match error {
            GetSimilarProductListingsError::NotFound => {
                ApiError::not_found(PRODUCT_LISTING_NOT_FOUND)
                    .with_detail("ProductListing was not found.")
            }
            GetSimilarProductListingsError::ProductListingEmbeddingQueryFailed { .. }
            | GetSimilarProductListingsError::SimilaritySearchUnavailable
            | GetSimilarProductListingsError::BeginTransactionFailed
            | GetSimilarProductListingsError::CommitTransactionFailed
            | GetSimilarProductListingsError::PricingFxSnapshotMissing
            | GetSimilarProductListingsError::PricingFxSnapshotUnavailable { .. }
            | GetSimilarProductListingsError::ListingSourceSummaryQueryFailed { .. }
            | GetSimilarProductListingsError::ProductListingUserStateQueryFailed { .. }
            | GetSimilarProductListingsError::ContentAssessmentQueryFailed { .. } => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Similar products are temporarily unavailable.")
            }
            GetSimilarProductListingsError::PricingFxSnapshotInvalid { .. }
            | GetSimilarProductListingsError::ListingSourceSummaryReadModelInvalid { .. }
            | GetSimilarProductListingsError::ListingSourceSummaryMissing { .. }
            | GetSimilarProductListingsError::ProductListingUserStateReadModelInvalid { .. }
            | GetSimilarProductListingsError::ProductListingUserStateMissing
            | GetSimilarProductListingsError::HiddenProductListingSummaryInvalid { .. }
            | GetSimilarProductListingsError::ContentAssessmentStateInvalid { .. } => {
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                    .with_detail("Similar product personalization failed internally.")
            }
        }
    }
}

impl From<SearchProductListingsError> for ApiError {
    fn from(error: SearchProductListingsError) -> Self {
        match error {
            SearchProductListingsError::ProductListingSearchQueryFailed
            | SearchProductListingsError::FxRateSnapshotMissing
            | SearchProductListingsError::BeginFxRateSnapshotTransactionFailed { .. }
            | SearchProductListingsError::FxRateSnapshotReadFailed { .. }
            | SearchProductListingsError::CommitFxRateSnapshotTransactionFailed { .. }
            | SearchProductListingsError::ListingSourceSummaryQueryFailed { .. }
            | SearchProductListingsError::ProductListingUserStateQueryFailed { .. }
            | SearchProductListingsError::ContentAssessmentQueryFailed { .. } => {
                ApiError::service_unavailable(PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("ProductListing search is temporarily unavailable.")
            }
            SearchProductListingsError::ProductListingSearchReadModelInvalid
            | SearchProductListingsError::FxRateSnapshotInvalid { .. }
            | SearchProductListingsError::ListingSourceSummaryReadModelInvalid { .. }
            | SearchProductListingsError::ListingSourceSummaryMissing { .. }
            | SearchProductListingsError::ProductListingUserStateReadModelInvalid { .. }
            | SearchProductListingsError::ProductListingUserStateMissing
            | SearchProductListingsError::HiddenProductListingSummaryInvalid { .. }
            | SearchProductListingsError::ContentAssessmentStateInvalid { .. } => {
                ApiError::internal_server_error(PRODUCT_LISTING_INTERNAL_ERROR)
                    .with_detail("ProductListing search failed internally.")
            }
        }
    }
}

impl From<UpsertNewsletterSubscriptionError> for ApiError {
    fn from(error: UpsertNewsletterSubscriptionError) -> Self {
        match error {
            UpsertNewsletterSubscriptionError::InvalidEmail => ApiError::bad_request(INVALID_EMAIL)
                .with_detail("Newsletter provider rejected the email address."),
            UpsertNewsletterSubscriptionError::NewsletterSubscriptionUnavailable { .. } => {
                ApiError::service_unavailable(NEWSLETTER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Newsletter subscription is temporarily unavailable.")
            }
            UpsertNewsletterSubscriptionError::NewsletterSubscriptionInternal { .. } => {
                ApiError::internal_server_error(NEWSLETTER_INTERNAL_ERROR)
                    .with_detail("Newsletter subscription failed internally.")
            }
        }
    }
}

impl From<CheckUserAdminError> for ApiError {
    fn from(error: CheckUserAdminError) -> Self {
        match error {
            CheckUserAdminError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CheckUserAdminError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CheckUserAdminError::TemporarilyUnavailable { .. }
            | CheckUserAdminError::BeginTransactionFailed
            | CheckUserAdminError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User details are temporarily unavailable.")
            }
            CheckUserAdminError::InvalidReadModel { .. } | CheckUserAdminError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User details failed internally.")
            }
        }
    }
}

impl From<GetOwnUserError> for ApiError {
    fn from(error: GetOwnUserError) -> Self {
        match error {
            GetOwnUserError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetOwnUserError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetOwnUserError::NotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            GetOwnUserError::TemporarilyUnavailable { .. }
            | GetOwnUserError::BeginTransactionFailed
            | GetOwnUserError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User details are temporarily unavailable.")
            }
            GetOwnUserError::InvalidReadModel { .. } | GetOwnUserError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User details failed internally.")
            }
        }
    }
}

impl From<AdminGetUserError> for ApiError {
    fn from(error: AdminGetUserError) -> Self {
        match error {
            AdminGetUserError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            AdminGetUserError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            AdminGetUserError::NotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            AdminGetUserError::TemporarilyUnavailable { .. }
            | AdminGetUserError::BeginTransactionFailed
            | AdminGetUserError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User details are temporarily unavailable.")
            }
            AdminGetUserError::InvalidReadModel { .. } | AdminGetUserError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User details failed internally.")
            }
        }
    }
}

impl From<CreatePartyError> for ApiError {
    fn from(error: CreatePartyError) -> Self {
        match error {
            CreatePartyError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreatePartyError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreatePartyError::SlugConflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Party conflicts with current state.")
            }
            CreatePartyError::TemporarilyUnavailable { .. }
            | CreatePartyError::BeginTransactionFailed
            | CreatePartyError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTY_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Party could not be created right now.")
            }
            CreatePartyError::InvalidPersistedState { .. } | CreatePartyError::Internal { .. } => {
                ApiError::internal_server_error(PARTY_INTERNAL_ERROR)
                    .with_detail("Party create failed internally.")
            }
        }
    }
}

impl From<UpdatePartyError> for ApiError {
    fn from(error: UpdatePartyError) -> Self {
        match error {
            UpdatePartyError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdatePartyError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdatePartyError::NotFound => {
                ApiError::not_found(PARTY_NOT_FOUND).with_detail("Party was not found.")
            }
            UpdatePartyError::ConcurrencyConflict | UpdatePartyError::SlugConflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Party conflicts with current state.")
            }
            UpdatePartyError::TemporarilyUnavailable { .. }
            | UpdatePartyError::BeginTransactionFailed
            | UpdatePartyError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTY_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Party could not be updated right now.")
            }
            UpdatePartyError::InvalidPersistedState { .. } | UpdatePartyError::Internal { .. } => {
                ApiError::internal_server_error(PARTY_INTERNAL_ERROR)
                    .with_detail("Party update failed internally.")
            }
        }
    }
}

impl From<GetPartyError> for ApiError {
    fn from(error: GetPartyError) -> Self {
        match error {
            GetPartyError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetPartyError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetPartyError::NotFound => {
                ApiError::not_found(PARTY_NOT_FOUND).with_detail("Party was not found.")
            }
            GetPartyError::TemporarilyUnavailable { .. }
            | GetPartyError::BeginTransactionFailed
            | GetPartyError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTY_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Party details are temporarily unavailable.")
            }
            GetPartyError::InvalidPersistedState { .. } | GetPartyError::Internal { .. } => {
                ApiError::internal_server_error(PARTY_INTERNAL_ERROR)
                    .with_detail("Party details failed internally.")
            }
        }
    }
}

impl From<SearchPartiesError> for ApiError {
    fn from(error: SearchPartiesError) -> Self {
        match error {
            SearchPartiesError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            SearchPartiesError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            SearchPartiesError::TemporarilyUnavailable { .. }
            | SearchPartiesError::BeginTransactionFailed
            | SearchPartiesError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTY_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Party search is temporarily unavailable.")
            }
            SearchPartiesError::InvalidReadModel { .. } | SearchPartiesError::Internal { .. } => {
                ApiError::internal_server_error(PARTY_INTERNAL_ERROR)
                    .with_detail("Party search failed internally.")
            }
        }
    }
}

impl From<SearchUsersError> for ApiError {
    fn from(error: SearchUsersError) -> Self {
        match error {
            SearchUsersError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            SearchUsersError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            SearchUsersError::TemporarilyUnavailable { .. }
            | SearchUsersError::BeginTransactionFailed
            | SearchUsersError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User search is temporarily unavailable.")
            }
            SearchUsersError::InvalidReadModel { .. } | SearchUsersError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User search failed internally.")
            }
        }
    }
}

impl From<UpdateUserProfileError> for ApiError {
    fn from(error: UpdateUserProfileError) -> Self {
        match error {
            UpdateUserProfileError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateUserProfileError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateUserProfileError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            UpdateUserProfileError::ConcurrencyConflict
            | UpdateUserProfileError::EmailConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User update conflicts with current state."),
            UpdateUserProfileError::EmailRequired
            | UpdateUserProfileError::InvalidUserState { .. } => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("User update is invalid.")
            }
            UpdateUserProfileError::TemporarilyUnavailable { .. }
            | UpdateUserProfileError::BeginTransactionFailed
            | UpdateUserProfileError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be updated right now.")
            }
            UpdateUserProfileError::InvalidPersistedState { .. }
            | UpdateUserProfileError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User update failed internally.")
            }
        }
    }
}

impl From<ChangeUserRoleError> for ApiError {
    fn from(error: ChangeUserRoleError) -> Self {
        match error {
            ChangeUserRoleError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ChangeUserRoleError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ChangeUserRoleError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            ChangeUserRoleError::LastAdminProtected => ApiError::conflict(CONFLICT)
                .with_detail("At least one active administrator must remain."),
            ChangeUserRoleError::ConcurrencyConflict
            | ChangeUserRoleError::EmailConflict { .. }
            | ChangeUserRoleError::StripeCustomerConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User update conflicts with current state."),
            ChangeUserRoleError::TemporarilyUnavailable { .. }
            | ChangeUserRoleError::BeginTransactionFailed
            | ChangeUserRoleError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be updated right now.")
            }
            ChangeUserRoleError::InvalidPersistedState { .. }
            | ChangeUserRoleError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User update failed internally.")
            }
        }
    }
}

impl From<ChangeUserTierError> for ApiError {
    fn from(error: ChangeUserTierError) -> Self {
        match error {
            ChangeUserTierError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ChangeUserTierError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ChangeUserTierError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            ChangeUserTierError::ConcurrencyConflict
            | ChangeUserTierError::EmailConflict { .. }
            | ChangeUserTierError::StripeCustomerConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User update conflicts with current state."),
            ChangeUserTierError::TemporarilyUnavailable { .. }
            | ChangeUserTierError::TierEntitlementsLockFailed { .. }
            | ChangeUserTierError::TierEntitlementsReconciliationFailed { .. }
            | ChangeUserTierError::BeginTransactionFailed
            | ChangeUserTierError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be updated right now.")
            }
            ChangeUserTierError::InvalidPersistedState { .. }
            | ChangeUserTierError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User update failed internally.")
            }
        }
    }
}

impl From<DeleteUserError> for ApiError {
    fn from(error: DeleteUserError) -> Self {
        match error {
            DeleteUserError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteUserError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteUserError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            DeleteUserError::LastAdminProtected => ApiError::conflict(CONFLICT)
                .with_detail("At least one active administrator must remain."),
            DeleteUserError::ConcurrencyConflict
            | DeleteUserError::EmailConflict { .. }
            | DeleteUserError::StripeCustomerConflict { .. } => ApiError::conflict(CONFLICT)
                .with_detail("User delete conflicts with current state."),
            DeleteUserError::TemporarilyUnavailable { .. }
            | DeleteUserError::BeginTransactionFailed
            | DeleteUserError::CommitTransactionFailed => {
                ApiError::service_unavailable(USER_TEMPORARILY_UNAVAILABLE)
                    .with_detail("User could not be deleted right now.")
            }
            DeleteUserError::InvalidPersistedState { .. } | DeleteUserError::Internal { .. } => {
                ApiError::internal_server_error(USER_INTERNAL_ERROR)
                    .with_detail("User delete failed internally.")
            }
        }
    }
}

impl From<CreateAccessTokenError> for ApiError {
    fn from(error: CreateAccessTokenError) -> Self {
        match error {
            CreateAccessTokenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateAccessTokenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateAccessTokenError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token already exists.")
            }
            CreateAccessTokenError::TemporarilyUnavailable { .. }
            | CreateAccessTokenError::BeginTransactionFailed
            | CreateAccessTokenError::CommitTransactionFailed => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            CreateAccessTokenError::InvalidPersistedState { .. }
            | CreateAccessTokenError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<ListAccessTokensError> for ApiError {
    fn from(error: ListAccessTokensError) -> Self {
        match error {
            ListAccessTokensError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListAccessTokensError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListAccessTokensError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token conflict.")
            }
            ListAccessTokensError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            ListAccessTokensError::InvalidPersistedState { .. }
            | ListAccessTokensError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<ListAdminAccessTokensError> for ApiError {
    fn from(error: ListAdminAccessTokensError) -> Self {
        match error {
            ListAdminAccessTokensError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListAdminAccessTokensError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListAdminAccessTokensError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            ListAdminAccessTokensError::TemporarilyUnavailable { .. }
            | ListAdminAccessTokensError::BeginTransactionFailed
            | ListAdminAccessTokensError::CommitTransactionFailed => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            ListAdminAccessTokensError::InvalidPersistedState { .. }
            | ListAdminAccessTokensError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<GetAccessTokenError> for ApiError {
    fn from(error: GetAccessTokenError) -> Self {
        match error {
            GetAccessTokenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetAccessTokenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetAccessTokenError::NotFound => ApiError::not_found(ACCESS_TOKEN_NOT_FOUND)
                .with_detail("Access token was not found."),
            GetAccessTokenError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token conflict.")
            }
            GetAccessTokenError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            GetAccessTokenError::InvalidPersistedState { .. }
            | GetAccessTokenError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<UpdateAccessTokenError> for ApiError {
    fn from(error: UpdateAccessTokenError) -> Self {
        match error {
            UpdateAccessTokenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateAccessTokenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateAccessTokenError::AccessTokenNotFound => {
                ApiError::not_found(ACCESS_TOKEN_NOT_FOUND)
                    .with_detail("Access token was not found.")
            }
            UpdateAccessTokenError::NameRequired => {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail("Access token name is required.")
            }
            UpdateAccessTokenError::Conflict { .. }
            | UpdateAccessTokenError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Access token conflict.")
            }
            UpdateAccessTokenError::TemporarilyUnavailable { .. }
            | UpdateAccessTokenError::BeginTransactionFailed
            | UpdateAccessTokenError::CommitTransactionFailed => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            UpdateAccessTokenError::InvalidPersistedState { .. }
            | UpdateAccessTokenError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<DeleteAccessTokensError> for ApiError {
    fn from(error: DeleteAccessTokensError) -> Self {
        match error {
            DeleteAccessTokensError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteAccessTokensError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteAccessTokensError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            DeleteAccessTokensError::TemporarilyUnavailable { .. }
            | DeleteAccessTokensError::BeginTransactionFailed
            | DeleteAccessTokensError::CommitTransactionFailed => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            DeleteAccessTokensError::InvalidPersistedState { .. }
            | DeleteAccessTokensError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}
impl From<DeleteAccessTokenError> for ApiError {
    fn from(error: DeleteAccessTokenError) -> Self {
        match error {
            DeleteAccessTokenError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            DeleteAccessTokenError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DeleteAccessTokenError::Conflict { .. } => {
                ApiError::conflict(CONFLICT).with_detail("Access token conflict.")
            }
            DeleteAccessTokenError::TemporarilyUnavailable { .. }
            | DeleteAccessTokenError::BeginTransactionFailed
            | DeleteAccessTokenError::CommitTransactionFailed => {
                ApiError::service_unavailable(ACCESS_TOKEN_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Access token store is temporarily unavailable.")
            }
            DeleteAccessTokenError::InvalidPersistedState { .. }
            | DeleteAccessTokenError::Internal { .. } => {
                ApiError::internal_server_error(ACCESS_TOKEN_INTERNAL_ERROR)
                    .with_detail("Access token operation failed internally.")
            }
        }
    }
}

impl From<ListWatchlistError> for ApiError {
    fn from(error: ListWatchlistError) -> Self {
        match error {
            ListWatchlistError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListWatchlistError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListWatchlistError::TemporarilyUnavailable
            | ListWatchlistError::CurrentPricingFxSnapshotMissing
            | ListWatchlistError::SalePricingFxSnapshotMissing { .. }
            | ListWatchlistError::PricingFxSnapshotUnavailable { .. }
            | ListWatchlistError::BeginTransactionFailed
            | ListWatchlistError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            ListWatchlistError::InvalidPersistedState
            | ListWatchlistError::PricingFxSnapshotInvalid { .. }
            | ListWatchlistError::SaleFxSnapshotMismatch { .. }
            | ListWatchlistError::ProductListingPriceConversionFailed { .. } => {
                ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                    .with_detail("Watchlist failed internally.")
            }
        }
    }
}
impl From<WatchProductListingError> for ApiError {
    fn from(error: WatchProductListingError) -> Self {
        match error {
            WatchProductListingError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            WatchProductListingError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            WatchProductListingError::AlreadyExists => {
                ApiError::conflict(CONFLICT).with_detail("Watchlist entry already exists.")
            }
            WatchProductListingError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            WatchProductListingError::WatchlistQuotaExceeded {
                active_count,
                quota,
            } => ApiError::unprocessable_content(WATCHLIST_QUOTA_EXCEEDED).with_detail(format!(
                "Exceeded the maximum amount of watchlist entries. There are already {active_count}/{quota} active watchlist entries occupied."
            )),
            WatchProductListingError::TemporarilyUnavailable { .. }
            | WatchProductListingError::UserTierEntitlementsLockFailed { .. }
            | WatchProductListingError::WatchlistQuotaReadFailed { .. }
            | WatchProductListingError::BeginTransactionFailed
            | WatchProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            WatchProductListingError::InvalidPersistedState => {
                ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                    .with_detail("Watchlist failed internally.")
            }
        }
    }
}
impl From<UpdateWatchlistProductListingError> for ApiError {
    fn from(error: UpdateWatchlistProductListingError) -> Self {
        match error {
            UpdateWatchlistProductListingError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UpdateWatchlistProductListingError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UpdateWatchlistProductListingError::NotFound => ApiError::not_found(WATCHLIST_ENTRY_NOT_FOUND)
                .with_detail("Watchlist entry was not found."),
            UpdateWatchlistProductListingError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Watchlist entry was changed concurrently.")
            }
            UpdateWatchlistProductListingError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            UpdateWatchlistProductListingError::WatchlistQuotaExceeded {
                active_count,
                quota,
            } => ApiError::unprocessable_content(WATCHLIST_QUOTA_EXCEEDED).with_detail(format!(
                "Exceeded the maximum amount of watchlist entries. There are already {active_count}/{quota} active watchlist entries occupied."
            )),
            UpdateWatchlistProductListingError::TemporarilyUnavailable { .. }
            | UpdateWatchlistProductListingError::UserTierEntitlementsLockFailed { .. }
            | UpdateWatchlistProductListingError::WatchlistQuotaReadFailed { .. }
            | UpdateWatchlistProductListingError::BeginTransactionFailed
            | UpdateWatchlistProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            UpdateWatchlistProductListingError::InvalidPersistedState => {
                ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                    .with_detail("Watchlist failed internally.")
            }
        }
    }
}
impl From<UnwatchProductListingError> for ApiError {
    fn from(error: UnwatchProductListingError) -> Self {
        match error {
            UnwatchProductListingError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            UnwatchProductListingError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            UnwatchProductListingError::NotFound => ApiError::not_found(WATCHLIST_ENTRY_NOT_FOUND)
                .with_detail("Watchlist entry was not found."),
            UnwatchProductListingError::ConcurrencyConflict => ApiError::conflict(CONFLICT)
                .with_detail("Watchlist entry was changed concurrently."),
            UnwatchProductListingError::TemporarilyUnavailable { .. }
            | UnwatchProductListingError::BeginTransactionFailed
            | UnwatchProductListingError::CommitTransactionFailed => {
                ApiError::service_unavailable(WATCHLIST_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Watchlist is temporarily unavailable.")
            }
            UnwatchProductListingError::InvalidPersistedState => {
                ApiError::internal_server_error(WATCHLIST_INTERNAL_ERROR)
                    .with_detail("Watchlist failed internally.")
            }
        }
    }
}

fn partnership_application_internal_error() -> ApiError {
    ApiError::internal_server_error(PARTNERSHIP_APPLICATION_INTERNAL_ERROR)
        .with_detail("Partnership application failed internally.")
}

fn partnership_application_temporarily_unavailable() -> ApiError {
    ApiError::service_unavailable(PARTNERSHIP_APPLICATION_TEMPORARILY_UNAVAILABLE)
        .with_detail("Partnership application is temporarily unavailable.")
}

fn partnership_application_forbidden() -> ApiError {
    ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
}

fn partnership_application_not_found() -> ApiError {
    ApiError::not_found(PARTNERSHIP_APPLICATION_NOT_FOUND)
        .with_detail("Partnership application was not found.")
}

impl From<SubmitPartnershipApplicationError> for ApiError {
    fn from(error: SubmitPartnershipApplicationError) -> Self {
        match error {
            SubmitPartnershipApplicationError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            SubmitPartnershipApplicationError::Forbidden => partnership_application_forbidden(),
            SubmitPartnershipApplicationError::TemporarilyUnavailable { .. }
            | SubmitPartnershipApplicationError::BeginTransactionFailed
            | SubmitPartnershipApplicationError::CommitTransactionFailed => {
                partnership_application_temporarily_unavailable()
            }
            SubmitPartnershipApplicationError::InvalidPersistedState { .. }
            | SubmitPartnershipApplicationError::Internal { .. } => {
                partnership_application_internal_error()
            }
        }
    }
}

impl From<ListOwnPartnershipApplicationsError> for ApiError {
    fn from(error: ListOwnPartnershipApplicationsError) -> Self {
        match error {
            ListOwnPartnershipApplicationsError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            ListOwnPartnershipApplicationsError::Forbidden => partnership_application_forbidden(),
            ListOwnPartnershipApplicationsError::TemporarilyUnavailable { .. }
            | ListOwnPartnershipApplicationsError::BeginTransactionFailed
            | ListOwnPartnershipApplicationsError::CommitTransactionFailed => {
                partnership_application_temporarily_unavailable()
            }
            ListOwnPartnershipApplicationsError::InvalidReadModel { .. }
            | ListOwnPartnershipApplicationsError::Internal { .. } => {
                partnership_application_internal_error()
            }
        }
    }
}

impl From<GetOwnPartnershipApplicationError> for ApiError {
    fn from(error: GetOwnPartnershipApplicationError) -> Self {
        match error {
            GetOwnPartnershipApplicationError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            GetOwnPartnershipApplicationError::Forbidden => partnership_application_forbidden(),
            GetOwnPartnershipApplicationError::NotFound => partnership_application_not_found(),
            GetOwnPartnershipApplicationError::TemporarilyUnavailable { .. }
            | GetOwnPartnershipApplicationError::BeginTransactionFailed
            | GetOwnPartnershipApplicationError::CommitTransactionFailed => {
                partnership_application_temporarily_unavailable()
            }
            GetOwnPartnershipApplicationError::InvalidPersistedState { .. }
            | GetOwnPartnershipApplicationError::Internal { .. } => {
                partnership_application_internal_error()
            }
        }
    }
}

impl From<WithdrawPartnershipApplicationError> for ApiError {
    fn from(error: WithdrawPartnershipApplicationError) -> Self {
        match error {
            WithdrawPartnershipApplicationError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            WithdrawPartnershipApplicationError::Forbidden => partnership_application_forbidden(),
            WithdrawPartnershipApplicationError::NotFound => partnership_application_not_found(),
            WithdrawPartnershipApplicationError::ApplicationNotWithdrawable
            | WithdrawPartnershipApplicationError::ConcurrencyConflict => ApiError::conflict(
                CONFLICT,
            )
            .with_detail("Partnership application cannot be withdrawn in its current state."),
            WithdrawPartnershipApplicationError::TemporarilyUnavailable { .. }
            | WithdrawPartnershipApplicationError::BeginTransactionFailed
            | WithdrawPartnershipApplicationError::CommitTransactionFailed => {
                partnership_application_temporarily_unavailable()
            }
            WithdrawPartnershipApplicationError::InvalidPersistedState { .. }
            | WithdrawPartnershipApplicationError::Internal { .. } => {
                partnership_application_internal_error()
            }
        }
    }
}

macro_rules! impl_partnership_application_admin_read_error {
    ($error:ident, $invalid:ident) => {
        impl From<$error> for ApiError {
            fn from(error: $error) -> Self {
                match error {
                    $error::Forbidden => partnership_application_forbidden(),
                    $error::TemporarilyUnavailable { .. }
                    | $error::BeginTransactionFailed
                    | $error::CommitTransactionFailed => {
                        partnership_application_temporarily_unavailable()
                    }
                    $error::$invalid { .. } | $error::Internal { .. } => {
                        partnership_application_internal_error()
                    }
                }
            }
        }
    };
}

impl_partnership_application_admin_read_error!(
    ListAdminPartnershipApplicationsError,
    InvalidReadModel
);

impl From<GetAdminPartnershipError> for ApiError {
    fn from(error: GetAdminPartnershipError) -> Self {
        match error {
            GetAdminPartnershipError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GetAdminPartnershipError::NotFound => {
                ApiError::not_found(PARTNERSHIP_NOT_FOUND).with_detail("Partnership was not found.")
            }
            GetAdminPartnershipError::TemporarilyUnavailable { .. }
            | GetAdminPartnershipError::BeginTransactionFailed
            | GetAdminPartnershipError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNERSHIP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partnership details are temporarily unavailable.")
            }
            GetAdminPartnershipError::InvalidReadModel { .. }
            | GetAdminPartnershipError::Internal { .. } => {
                ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                    .with_detail("Partnership details failed internally.")
            }
        }
    }
}

impl From<DissolvePartnershipError> for ApiError {
    fn from(error: DissolvePartnershipError) -> Self {
        match error {
            DissolvePartnershipError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            DissolvePartnershipError::PartnershipNotFound => {
                ApiError::not_found(PARTNERSHIP_NOT_FOUND).with_detail("Partnership was not found.")
            }
            DissolvePartnershipError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Partnership was changed concurrently.")
            }
            DissolvePartnershipError::TemporarilyUnavailable { .. }
            | DissolvePartnershipError::BeginTransactionFailed
            | DissolvePartnershipError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNERSHIP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partnership dissolution is temporarily unavailable.")
            }
            DissolvePartnershipError::InvalidPersistedState { .. }
            | DissolvePartnershipError::Internal { .. } => {
                ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                    .with_detail("Partnership dissolution failed internally.")
            }
        }
    }
}

impl From<GrantPartnershipListingSourceError> for ApiError {
    fn from(error: GrantPartnershipListingSourceError) -> Self {
        match error {
            GrantPartnershipListingSourceError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GrantPartnershipListingSourceError::PartnershipNotFound => {
                ApiError::not_found(PARTNERSHIP_NOT_FOUND).with_detail("Partnership was not found.")
            }
            GrantPartnershipListingSourceError::ListingSourceNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            GrantPartnershipListingSourceError::PartnershipPartyMismatch => {
                ApiError::conflict(CONFLICT)
                    .with_detail("Partnership and ListingSource belong to different Parties.")
            }
            GrantPartnershipListingSourceError::TemporarilyUnavailable { .. }
            | GrantPartnershipListingSourceError::BeginTransactionFailed
            | GrantPartnershipListingSourceError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNERSHIP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partnership ListingSource grant is temporarily unavailable.")
            }
            GrantPartnershipListingSourceError::InvalidPersistedState { .. }
            | GrantPartnershipListingSourceError::Internal { .. } => {
                ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                    .with_detail("Partnership ListingSource grant failed internally.")
            }
        }
    }
}

impl From<RevokePartnershipListingSourceError> for ApiError {
    fn from(error: RevokePartnershipListingSourceError) -> Self {
        match error {
            RevokePartnershipListingSourceError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            RevokePartnershipListingSourceError::PartnershipNotFound => {
                ApiError::not_found(PARTNERSHIP_NOT_FOUND).with_detail("Partnership was not found.")
            }
            RevokePartnershipListingSourceError::ListingSourceNotFound => {
                ApiError::not_found(LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            RevokePartnershipListingSourceError::TemporarilyUnavailable { .. }
            | RevokePartnershipListingSourceError::BeginTransactionFailed
            | RevokePartnershipListingSourceError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNERSHIP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partnership ListingSource grant is temporarily unavailable.")
            }
            RevokePartnershipListingSourceError::InvalidPersistedState { .. }
            | RevokePartnershipListingSourceError::Internal { .. } => {
                ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                    .with_detail("Partnership ListingSource grant failed internally.")
            }
        }
    }
}

impl From<GrantPartnershipMembershipError> for ApiError {
    fn from(error: GrantPartnershipMembershipError) -> Self {
        match error {
            GrantPartnershipMembershipError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            GrantPartnershipMembershipError::PartnershipNotFound => {
                ApiError::not_found(PARTNERSHIP_NOT_FOUND).with_detail("Partnership was not found.")
            }
            GrantPartnershipMembershipError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            GrantPartnershipMembershipError::TemporarilyUnavailable { .. }
            | GrantPartnershipMembershipError::BeginTransactionFailed
            | GrantPartnershipMembershipError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNERSHIP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partnership membership is temporarily unavailable.")
            }
            GrantPartnershipMembershipError::InvalidPersistedState { .. }
            | GrantPartnershipMembershipError::Internal { .. } => {
                ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                    .with_detail("Partnership membership failed internally.")
            }
        }
    }
}

impl From<RevokePartnershipMembershipError> for ApiError {
    fn from(error: RevokePartnershipMembershipError) -> Self {
        match error {
            RevokePartnershipMembershipError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            RevokePartnershipMembershipError::PartnershipNotFound => {
                ApiError::not_found(PARTNERSHIP_NOT_FOUND).with_detail("Partnership was not found.")
            }
            RevokePartnershipMembershipError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            RevokePartnershipMembershipError::TemporarilyUnavailable { .. }
            | RevokePartnershipMembershipError::BeginTransactionFailed
            | RevokePartnershipMembershipError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNERSHIP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partnership membership is temporarily unavailable.")
            }
            RevokePartnershipMembershipError::InvalidPersistedState { .. }
            | RevokePartnershipMembershipError::Internal { .. } => {
                ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                    .with_detail("Partnership membership failed internally.")
            }
        }
    }
}

impl From<ListAdminPartnershipsError> for ApiError {
    fn from(error: ListAdminPartnershipsError) -> Self {
        match error {
            ListAdminPartnershipsError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            ListAdminPartnershipsError::TemporarilyUnavailable { .. }
            | ListAdminPartnershipsError::BeginTransactionFailed
            | ListAdminPartnershipsError::CommitTransactionFailed => {
                ApiError::service_unavailable(PARTNERSHIP_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Partnerships are temporarily unavailable.")
            }
            ListAdminPartnershipsError::InvalidReadModel { .. }
            | ListAdminPartnershipsError::Internal { .. } => {
                ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                    .with_detail("Partnership search failed internally.")
            }
        }
    }
}

impl From<GetPartnershipApplicationError> for ApiError {
    fn from(error: GetPartnershipApplicationError) -> Self {
        match error {
            GetPartnershipApplicationError::Forbidden => partnership_application_forbidden(),
            GetPartnershipApplicationError::NotFound => partnership_application_not_found(),
            GetPartnershipApplicationError::TemporarilyUnavailable { .. }
            | GetPartnershipApplicationError::BeginTransactionFailed
            | GetPartnershipApplicationError::CommitTransactionFailed => {
                partnership_application_temporarily_unavailable()
            }
            GetPartnershipApplicationError::InvalidPersistedState { .. }
            | GetPartnershipApplicationError::Internal { .. } => {
                partnership_application_internal_error()
            }
        }
    }
}

macro_rules! impl_partnership_application_admin_transition_error {
    ($error:ident, $invalid_state:ident) => {
        impl From<$error> for ApiError {
            fn from(error: $error) -> Self {
                match error {
                    $error::Forbidden => partnership_application_forbidden(),
                    $error::NotFound => partnership_application_not_found(),
                    $error::$invalid_state | $error::ConcurrencyConflict => ApiError::conflict(
                        CONFLICT,
                    )
                    .with_detail("Partnership application cannot change in its current state."),
                    $error::TemporarilyUnavailable { .. }
                    | $error::BeginTransactionFailed
                    | $error::CommitTransactionFailed => {
                        partnership_application_temporarily_unavailable()
                    }
                    $error::InvalidPersistedState { .. } | $error::Internal { .. } => {
                        partnership_application_internal_error()
                    }
                }
            }
        }
    };
}

impl_partnership_application_admin_transition_error!(
    MarkPartnershipApplicationInReviewError,
    ApplicationNotReviewable
);

impl From<ApprovePartnershipApplicationError> for ApiError {
    fn from(error: ApprovePartnershipApplicationError) -> Self {
        match error {
            ApprovePartnershipApplicationError::Forbidden => partnership_application_forbidden(),
            ApprovePartnershipApplicationError::NotFound => partnership_application_not_found(),
            ApprovePartnershipApplicationError::ListingSourceNotFound => {
                ApiError::not_found(PARTNERSHIP_APPLICATION_LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            ApprovePartnershipApplicationError::ApplicationNotApprovable
            | ApprovePartnershipApplicationError::ConcurrencyConflict
            | ApprovePartnershipApplicationError::SlugConflict { .. } => {
                ApiError::conflict(CONFLICT)
                    .with_detail("Partnership application cannot be approved in its current state.")
            }
            ApprovePartnershipApplicationError::NotificationCreateFailed { .. }
            | ApprovePartnershipApplicationError::TemporarilyUnavailable { .. }
            | ApprovePartnershipApplicationError::BeginTransactionFailed
            | ApprovePartnershipApplicationError::CommitTransactionFailed => {
                partnership_application_temporarily_unavailable()
            }
            ApprovePartnershipApplicationError::InvalidPersistedState { .. }
            | ApprovePartnershipApplicationError::Internal { .. } => {
                partnership_application_internal_error()
            }
        }
    }
}

impl From<RejectPartnershipApplicationError> for ApiError {
    fn from(error: RejectPartnershipApplicationError) -> Self {
        match error {
            RejectPartnershipApplicationError::Forbidden => partnership_application_forbidden(),
            RejectPartnershipApplicationError::NotFound => partnership_application_not_found(),
            RejectPartnershipApplicationError::ListingSourceNotFound => {
                ApiError::not_found(PARTNERSHIP_APPLICATION_LISTING_SOURCE_NOT_FOUND)
                    .with_detail("Listing source was not found.")
            }
            RejectPartnershipApplicationError::ApplicationNotRejectable
            | RejectPartnershipApplicationError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT)
                    .with_detail("Partnership application cannot be rejected in its current state.")
            }
            RejectPartnershipApplicationError::NotificationCreateFailed { .. }
            | RejectPartnershipApplicationError::TemporarilyUnavailable { .. }
            | RejectPartnershipApplicationError::BeginTransactionFailed
            | RejectPartnershipApplicationError::CommitTransactionFailed => {
                partnership_application_temporarily_unavailable()
            }
            RejectPartnershipApplicationError::InvalidPersistedState { .. }
            | RejectPartnershipApplicationError::Internal { .. } => {
                partnership_application_internal_error()
            }
        }
    }
}

impl From<OAuthServiceError> for ApiError {
    fn from(error: OAuthServiceError) -> Self {
        match error {
            OAuthServiceError::AuthenticatedActorRequired
            | OAuthServiceError::InvalidClientSecret => {
                ApiError::unauthorized(OAUTH_INVALID_CLIENT_SECRET).with_detail(error.to_string())
            }
            OAuthServiceError::Forbidden => ApiError::forbidden(FORBIDDEN),
            OAuthServiceError::ClientNotFound => ApiError::not_found(OAUTH_CLIENT_NOT_FOUND),
            OAuthServiceError::ConcurrencyConflict => ApiError::conflict(CONFLICT),
            OAuthServiceError::InvalidRedirectUri => {
                ApiError::bad_request(OAUTH_INVALID_REDIRECT_URI)
            }
            OAuthServiceError::InvalidScope => ApiError::bad_request(OAUTH_INVALID_SCOPE),
            OAuthServiceError::AuthorizationCodeNotFound => {
                ApiError::bad_request(OAUTH_AUTHORIZATION_CODE_NOT_FOUND)
            }
            OAuthServiceError::AuthorizationCodeExpired => {
                ApiError::bad_request(OAUTH_AUTHORIZATION_CODE_EXPIRED)
            }
            OAuthServiceError::AuthorizationCodeClientMismatch
            | OAuthServiceError::AuthorizationCodeRedirectUriMismatch => {
                ApiError::bad_request(OAUTH_AUTHORIZATION_CODE_NOT_FOUND)
            }
            OAuthServiceError::InvalidCodeVerifier => {
                ApiError::bad_request(OAUTH_INVALID_CODE_VERIFIER)
            }
            OAuthServiceError::ThirdPartyExchangeCodeNotFound
            | OAuthServiceError::ThirdPartyExchangeCodeExpired => {
                ApiError::bad_request(OAUTH_THIRD_PARTY_EXCHANGE_CODE_NOT_FOUND)
            }
            OAuthServiceError::InvalidClientMetadata(detail) => {
                ApiError::bad_request(OAUTH_INVALID_CLIENT_METADATA).with_detail(detail)
            }
            OAuthServiceError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(OAUTH_TEMPORARILY_UNAVAILABLE)
            }
            OAuthServiceError::InvalidPersistedState { .. }
            | OAuthServiceError::Internal { .. } => {
                ApiError::internal_server_error(OAUTH_INTERNAL_ERROR)
            }
        }
    }
}

impl From<CreateBillingCheckoutSessionError> for ApiError {
    fn from(error: CreateBillingCheckoutSessionError) -> Self {
        match error {
            CreateBillingCheckoutSessionError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateBillingCheckoutSessionError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateBillingCheckoutSessionError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            CreateBillingCheckoutSessionError::StripeCustomerAlreadyExists => {
                ApiError::conflict(STRIPE_CUSTOMER_ALREADY_EXISTS)
                    .with_detail("A Stripe customer is already associated with this user.")
            }
            CreateBillingCheckoutSessionError::StripeCustomerAssociationConflict => {
                ApiError::conflict(STRIPE_CUSTOMER_ASSOCIATION_CONFLICT)
                    .with_detail("Stripe customer association conflicts with current user state.")
            }
            CreateBillingCheckoutSessionError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Billing state changed concurrently.")
            }
            CreateBillingCheckoutSessionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(BILLING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Billing is temporarily unavailable.")
            }
            CreateBillingCheckoutSessionError::ProviderRejected { .. }
            | CreateBillingCheckoutSessionError::ProviderInvalidResponse { .. } => {
                ApiError::internal_server_error(BILLING_PROVIDER_FAILURE)
                    .with_detail("Billing provider could not create a session.")
            }
            CreateBillingCheckoutSessionError::Internal { .. } => {
                ApiError::internal_server_error(BILLING_INTERNAL_ERROR)
                    .with_detail("Billing failed internally.")
            }
        }
    }
}

impl From<CreateBillingPortalSessionError> for ApiError {
    fn from(error: CreateBillingPortalSessionError) -> Self {
        match error {
            CreateBillingPortalSessionError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateBillingPortalSessionError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateBillingPortalSessionError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            CreateBillingPortalSessionError::StripeCustomerDoesNotExist => {
                ApiError::unprocessable_content(STRIPE_CUSTOMER_DOES_NOT_EXIST)
                    .with_detail("No Stripe customer is associated with this user.")
            }
            CreateBillingPortalSessionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(BILLING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Billing is temporarily unavailable.")
            }
            CreateBillingPortalSessionError::ProviderRejected { .. }
            | CreateBillingPortalSessionError::ProviderInvalidResponse { .. } => {
                ApiError::internal_server_error(BILLING_PROVIDER_FAILURE)
                    .with_detail("Billing provider could not create a session.")
            }
            CreateBillingPortalSessionError::Internal { .. } => {
                ApiError::internal_server_error(BILLING_INTERNAL_ERROR)
                    .with_detail("Billing failed internally.")
            }
        }
    }
}

impl From<CreateBillingManagementSessionError> for ApiError {
    fn from(error: CreateBillingManagementSessionError) -> Self {
        match error {
            CreateBillingManagementSessionError::AuthenticatedActorRequired => {
                ApiError::unauthorized(INVALID_CREDENTIALS)
                    .with_header_field("Authorization")
                    .with_detail("Bearer token is required.")
            }
            CreateBillingManagementSessionError::Forbidden => {
                ApiError::forbidden(FORBIDDEN).with_detail("Operation is not permitted.")
            }
            CreateBillingManagementSessionError::UserNotFound => {
                ApiError::not_found(USER_NOT_FOUND).with_detail("User was not found.")
            }
            CreateBillingManagementSessionError::StripeCustomerDoesNotExist => {
                ApiError::unprocessable_content(STRIPE_CUSTOMER_DOES_NOT_EXIST)
                    .with_detail("No Stripe customer is associated with this user.")
            }
            CreateBillingManagementSessionError::StripeCustomerAssociationConflict => {
                ApiError::conflict(STRIPE_CUSTOMER_ASSOCIATION_CONFLICT)
                    .with_detail("Stripe customer association conflicts with current user state.")
            }
            CreateBillingManagementSessionError::ConcurrencyConflict => {
                ApiError::conflict(CONFLICT).with_detail("Billing state changed concurrently.")
            }
            CreateBillingManagementSessionError::TemporarilyUnavailable { .. } => {
                ApiError::service_unavailable(BILLING_TEMPORARILY_UNAVAILABLE)
                    .with_detail("Billing is temporarily unavailable.")
            }
            CreateBillingManagementSessionError::ProviderRejected { .. }
            | CreateBillingManagementSessionError::ProviderInvalidResponse { .. } => {
                ApiError::internal_server_error(BILLING_PROVIDER_FAILURE)
                    .with_detail("Billing provider could not create a session.")
            }
            CreateBillingManagementSessionError::Internal { .. } => {
                ApiError::internal_server_error(BILLING_INTERNAL_ERROR)
                    .with_detail("Billing failed internally.")
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status_code(),
            [(header::CONTENT_TYPE, "application/problem+json")],
            Json(self),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn should_map_temporary_jwks_failure_to_service_unavailable()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = ApiError::from(AuthError::TemporarilyUnavailable).into_response();

        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(AUTH_TEMPORARILY_UNAVAILABLE.to_string(), body["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_update_watchlist_concurrency_conflict_to_conflict()
    -> Result<(), Box<dyn std::error::Error>> {
        let response =
            ApiError::from(UpdateWatchlistProductListingError::ConcurrencyConflict).into_response();

        assert_eq!(StatusCode::CONFLICT, response.status());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(CONFLICT.to_string(), body["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_unwatch_product_concurrency_conflict_to_conflict()
    -> Result<(), Box<dyn std::error::Error>> {
        let response =
            ApiError::from(UnwatchProductListingError::ConcurrencyConflict).into_response();

        assert_eq!(StatusCode::CONFLICT, response.status());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(CONFLICT.to_string(), body["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_update_party_concurrency_conflict_to_conflict()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = ApiError::from(UpdatePartyError::ConcurrencyConflict).into_response();

        assert_eq!(StatusCode::CONFLICT, response.status());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(CONFLICT.to_string(), body["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_update_listing_source_concurrency_conflict_to_conflict()
    -> Result<(), Box<dyn std::error::Error>> {
        let response =
            ApiError::from(UpdateListingSourceError::ConcurrencyConflict).into_response();

        assert_eq!(StatusCode::CONFLICT, response.status());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(CONFLICT.to_string(), body["error"]);
        Ok(())
    }

    #[tokio::test]
    async fn should_render_problem_json_response() -> Result<(), Box<dyn std::error::Error>> {
        let response = ApiError::bad_request(INVALID_UUID)
            .with_path_field("shopId")
            .with_detail("Path parameter 'shopId' must be a UUID.")
            .into_response();

        assert_eq!(StatusCode::BAD_REQUEST, response.status());
        assert_eq!(
            "application/problem+json",
            response.headers()[header::CONTENT_TYPE]
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body = serde_json::from_slice::<serde_json::Value>(&bytes)?;
        assert_eq!(
            json!({
                "status": 400,
                "title": "Bad Request",
                "error": INVALID_UUID.to_string(),
                "source": {"field": "shopId", "type": "PATH"},
                "detail": "Path parameter 'shopId' must be a UUID."
            }),
            body
        );
        Ok(())
    }
}

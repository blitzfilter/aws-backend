pub(crate) mod admin_overview;
pub mod auth;
pub mod billing;
pub mod error;
pub mod listing_sources;
pub mod newsletter;
pub mod notifications;
pub mod oauth;
pub(crate) mod pagination_data;
pub mod parties;
pub mod partner_product_listings;
pub(crate) mod partnership_applications;
pub(crate) mod partnerships;
pub(crate) mod patch_value;
pub mod product_listings;
pub mod search_filters;
pub mod state;
pub mod transport;
pub mod users;
pub(crate) mod values;
pub mod watchlist;
pub mod webhooks;
pub(crate) mod wire;

use crate::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, CognitoJwtAuthenticator,
    CognitoJwtConfig, JwksProvider, ReqwestJwksProvider, TokenAuthenticator,
    UserAuthenticationAuthenticator,
};
use crate::state::{
    AdminOverviewState, AppState, BillingState, ListingSourcesState, NewsletterState,
    NotificationsState, OAuthState, PartiesState, PartnerProductListingsState,
    PartnershipApplicationsState, PartnershipsState, ProductListingsState, ReadinessCheck,
    SearchFiltersState, UsersState, WatchlistState, WebhooksState,
};
use crate::transport::with_transport_middleware;
use admin_overview_postgres::SqlxAdminOverviewReaderFactory;
use admin_overview_service::GetAdminOverviewHandler;
use axum::Router;
use axum::routing::{delete, get, patch, post};
use billing_service::use_cases::{
    BillingPriceIds, CreateBillingCheckoutSessionHandler, CreateBillingManagementSessionHandler,
    CreateBillingPortalSessionHandler,
};
use billing_stripe::{StripeBillingClient, StripeBillingConfig};
use embedding::{EmbeddingGenerator, VertexAiEmbeddingConfig, VertexAiEmbeddingGenerator};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use notification_postgres::{
    SqlxNotificationDeleter, SqlxNotificationDeliveryIntentRepositoryFactory,
    SqlxNotificationListReader, SqlxNotificationRepositoryFactory, SqlxNotificationSeenWriter,
};
use notification_service::use_cases::commands::delete_notification::DeleteNotificationHandler;
use notification_service::use_cases::commands::delete_notifications::DeleteNotificationsHandler;
use notification_service::use_cases::commands::update_all_notifications_seen::UpdateAllNotificationsSeenHandler;
use notification_service::use_cases::commands::update_notification_seen::UpdateNotificationSeenHandler;
use notification_service::use_cases::commands::update_notifications_seen::UpdateNotificationsSeenHandler;
use notification_service::use_cases::queries::list_notifications::ListNotificationsHandler;
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use oauth_postgres::{
    SqlxAuthorizationCodeRepositoryFactory, SqlxOAuthClientAuthenticationReader,
    SqlxOAuthClientDetailsReader, SqlxOAuthClientListReader, SqlxOAuthClientRepositoryFactory,
    SqlxThirdPartyExchangeCodeRepositoryFactory,
};
use oauth_service::use_cases::{
    AuthorizeHandler, CreateOAuthClientHandler, DeleteOAuthClientHandler, GetOAuthClientHandler,
    IntrospectTokenHandler, ListOAuthClientsHandler, RevokeTokenHandler,
    TokenByAuthorizationCodeHandler, TokenByThirdPartyCodeHandler, UpdateOAuthClientHandler,
};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_postgres::{PostgresConnectError, PostgresPoolConfig, SqlxUnitOfWork};
use woocommerce_service::WoocommerceWebhookIntake;

use listing_source_postgres::{
    SqlxListingSourceReaders, SqlxListingSourceRepositoryFactory,
    SqlxListingSourceSearchReaderFactory,
};
use listing_source_service::use_cases::commands::create_listing_source::CreateListingSourceHandler;
use listing_source_service::use_cases::commands::delete_listing_source::DeleteListingSourceHandler;
use listing_source_service::use_cases::commands::update_listing_source::UpdateListingSourceHandler;
use listing_source_service::use_cases::queries::get_listing_source::GetListingSourceHandler;
use listing_source_service::use_cases::queries::search_listing_sources::SearchListingSourcesHandler;
use partnership_postgres::{
    SqlxListingSourceAuthorization, SqlxListingSourceGrantRepositoryFactory,
    SqlxPartnershipApplicationReaderFactory, SqlxPartnershipApplicationRepositoryFactory,
    SqlxPartnershipDetailsReaderFactory, SqlxPartnershipRepositoryFactory,
    SqlxPartnershipSearchReaderFactory,
};
use partnership_service::use_cases::{
    commands::{
        approve_partnership_application::ApprovePartnershipApplicationHandler,
        dissolve_partnership::DissolvePartnershipHandler,
        grant_partnership_listing_source::GrantPartnershipListingSourceHandler,
        grant_partnership_membership::GrantPartnershipMembershipHandler,
        mark_partnership_application_in_review::MarkPartnershipApplicationInReviewHandler,
        reject_partnership_application::RejectPartnershipApplicationHandler,
        revoke_partnership_listing_source::RevokePartnershipListingSourceHandler,
        revoke_partnership_membership::RevokePartnershipMembershipHandler,
        submit_partnership_application::SubmitPartnershipApplicationHandler,
        withdraw_partnership_application::WithdrawPartnershipApplicationHandler,
    },
    queries::{
        get_admin_partnership::GetAdminPartnershipHandler,
        get_own_partnership_application::GetOwnPartnershipApplicationHandler,
        get_partnership_application::GetPartnershipApplicationHandler,
        list_admin_partnership_applications::ListAdminPartnershipApplicationsHandler,
        list_admin_partnerships::ListAdminPartnershipsHandler,
        list_administered_listing_sources::ListAdministeredListingSourcesHandler,
        list_own_partnership_applications::ListOwnPartnershipApplicationsHandler,
    },
};
use party_postgres::{SqlxPartyRepositoryFactory, SqlxPartySearchReaderFactory};
use party_service::use_cases::commands::create_party::CreatePartyHandler;
use party_service::use_cases::commands::delete_party::DeletePartyHandler;
use party_service::use_cases::commands::update_party::UpdatePartyHandler;
use party_service::use_cases::queries::get_party::GetPartyHandler;
use party_service::use_cases::queries::search_parties::SearchPartiesHandler;
use product_listing_opensearch::{
    OpenSearchProductListingSearchReader, OpenSearchProductListingSimilarProductListingsReader,
};
use product_listing_postgres::{
    SqlxListingSourceSummaryReader, SqlxPartnerProductListingAuthorizerFactory,
    SqlxProductListingContentAssessmentReader, SqlxProductListingDetailsBatchReader,
    SqlxProductListingDetailsReaderFactory, SqlxProductListingEmbeddingReaderFactory,
    SqlxProductListingEventAppenderFactory, SqlxProductListingHistoryReaderFactory,
    SqlxProductListingLifecycleGuardFactory, SqlxProductListingRawCaptureWriterFactory,
    SqlxProductListingRepositoryFactory, SqlxProductListingUserStateReader,
    SqlxProductListingWatchlistDetailsReaderFactory,
};
use product_listing_service::use_cases::{
    AuthorizeProductListingRawCaptureHandler, CaptureProductListingRawObservationHandler,
    CreateProductListingHandler, GetProductListingHandler, GetProductListingHistoryHandler,
    GetSimilarProductListingsHandler, ProductListingSearchReadExecutionPolicy,
    SearchProductListingsHandler, UpdateProductListingHandler, UpsertProductListingHandler,
    WithdrawProductListingHandler,
};
use search_filter_postgres::{
    SqlxSearchFilterMatchRepositoryFactory, SqlxSearchFilterQuotaReaderFactory,
    SqlxSearchFilterReader, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::use_cases::{
    CreateSearchFilterHandler, DeleteOwnedSearchFilterHandler, GetOwnedSearchFilterHandler,
    ListOwnedSearchFiltersHandler, ListSearchFilterMatchesHandler, UpdateOwnedSearchFilterHandler,
    UpdateSearchFilterMatchFeedbackHandler,
};

use sqlx::PgPool;
use std::future::Future;
use std::net::{AddrParseError, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::info;
use user_cognito::CognitoUserSessionRevoker;
use user_postgres::{
    SqlxAccessTokenAuthenticationReader, SqlxAccessTokenDetailsReader, SqlxAccessTokenListReader,
    SqlxAccessTokenRepositoryFactory, SqlxAdminAccessTokenListReaderFactory,
    SqlxCognitoUserIdentityReader, SqlxNewsletterProfileReader, SqlxUserAccountReaderFactory,
    SqlxUserAdminReaderFactory, SqlxUserAuthenticationReader,
    SqlxUserCognitoIdentityRegistryFactory, SqlxUserRepositoryFactory, SqlxUserSearchReaderFactory,
    SqlxUserTierEntitlementsFactory,
};
use user_service::use_cases::commands::associate_user_stripe_customer_id::AssociateUserStripeCustomerIdHandler;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleHandler;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierHandler;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenHandler;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenHandler;
use user_service::use_cases::commands::delete_access_tokens::DeleteAccessTokensHandler;
use user_service::use_cases::commands::delete_user::DeleteUserHandler;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenHandler;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileHandler;
use user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionHandler;
use user_service::use_cases::queries::admin_get_user::AdminGetUserHandler;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;
use user_service::use_cases::queries::get_access_token::GetAccessTokenHandler;
use user_service::use_cases::queries::get_own_user::GetOwnUserHandler;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensHandler;
use user_service::use_cases::queries::list_admin_access_tokens::ListAdminAccessTokensHandler;
use user_service::use_cases::queries::search_users::SearchUsersHandler;
use user_service::use_cases::{
    AuthenticateAccessTokenHandler, AuthenticateUserHandler, ResolveCognitoUserHandler,
    RevokeUserSessionsHandler, SuspendUserHandler, UnsuspendUserHandler,
};
use user_zoho::ZohoNewsletterSubscriptionWriter;
use watchlist_postgres::{SqlxWatchlistQuotaReaderFactory, SqlxWatchlistRepositoryFactory};
use watchlist_service::use_cases::{
    ListWatchlistHandler, UnwatchProductListingHandler, UpdateWatchlistProductListingHandler,
    WatchProductListingHandler,
};

pub const API_BIND_ADDR_ENV: &str = "AURA_HISTORIA_API_BIND_ADDR";
pub const VERTEX_AI_PROJECT_ID_ENV: &str = "VERTEX_AI_PROJECT_ID";
pub const VERTEX_AI_LOCATION_ENV: &str = "VERTEX_AI_LOCATION";
pub const COGNITO_ISSUER_ENV: &str = "AURA_HISTORIA_COGNITO_ISSUER";
pub const COGNITO_JWKS_URL_ENV: &str = "AURA_HISTORIA_COGNITO_JWKS_URL";
pub const COGNITO_APP_CLIENT_IDS_ENV: &str = "AURA_HISTORIA_COGNITO_APP_CLIENT_IDS";
pub const COGNITO_USER_POOL_ID_ENV: &str = "AURA_HISTORIA_COGNITO_USER_POOL_ID";
pub const STRIPE_API_KEY_ENV: &str = "STRIPE_API_KEY";
pub const STRIPE_CHECKOUT_SUCCESS_URL_ENV: &str = "STRIPE_CHECKOUT_SUCCESS_URL";
pub const STRIPE_CHECKOUT_CANCEL_URL_ENV: &str = "STRIPE_CHECKOUT_CANCEL_URL";
pub const STRIPE_PORTAL_RETURN_URL_ENV: &str = "STRIPE_PORTAL_RETURN_URL";
pub const STRIPE_PRO_MONTHLY_PRICE_ID_ENV: &str = "STRIPE_PRO_MONTHLY_PRICE_ID";
pub const STRIPE_PRO_YEARLY_PRICE_ID_ENV: &str = "STRIPE_PRO_YEARLY_PRICE_ID";
pub const STRIPE_ULTIMATE_MONTHLY_PRICE_ID_ENV: &str = "STRIPE_ULTIMATE_MONTHLY_PRICE_ID";
pub const STRIPE_ULTIMATE_YEARLY_PRICE_ID_ENV: &str = "STRIPE_ULTIMATE_YEARLY_PRICE_ID";
pub const ZOHO_LIST_KEY_ENV: &str = "ZOHO_LIST_KEY";
pub const ZOHO_CLIENT_ID_ENV: &str = "ZOHO_CLIENT_ID";
pub const ZOHO_CLIENT_SECRET_ENV: &str = "ZOHO_CLIENT_SECRET";
pub const ZOHO_REFRESH_TOKEN_ENV: &str = "ZOHO_REFRESH_TOKEN";
pub const ZOHO_ACCOUNTS_URL_ENV: &str = "ZOHO_ACCOUNTS_URL";
pub const ZOHO_CAMPAIGNS_URL_ENV: &str = "ZOHO_CAMPAIGNS_URL";
pub const PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV: &str =
    "PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED";
const POSTGRES_HOST_ENV: &str = "POSTGRES_HOST";
const POSTGRES_PORT_ENV: &str = "POSTGRES_PORT";
const POSTGRES_DATABASE_ENV: &str = "POSTGRES_DATABASE";
const POSTGRES_USERNAME_ENV: &str = "POSTGRES_USERNAME";
const POSTGRES_PASSWORD_ENV: &str = "POSTGRES_PASSWORD";
const POSTGRES_MAX_CONNECTIONS_ENV: &str = "POSTGRES_MAX_CONNECTIONS";
const DEFAULT_POSTGRES_PORT: u16 = 5432;
const DEFAULT_POSTGRES_MAX_CONNECTIONS: u32 = 2;
const DEFAULT_API_BIND_ADDR: &str = "0.0.0.0:8080";
const JWKS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const JWKS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_VERTEX_AI_PROJECT_ID: &str = "project-2c6e1dcc-3fb9-4910-adc";
const DEFAULT_VERTEX_AI_LOCATION: &str = "eu";
const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[derive(Clone, PartialEq, Eq)]
pub struct ApiConfig {
    bind_addr: SocketAddr,
    cognito_jwt: CognitoJwtConfig,
    cognito_user_pool_id: String,
    vertex_ai_embedding: VertexAiEmbeddingConfig,
    stripe_billing: StripeBillingConfig,
    billing_prices: BillingPriceIds,
    zoho: ZohoConfig,
    product_listing_search_parallel_enrichment_enabled: bool,
}

impl ApiConfig {
    pub fn from_env() -> Result<Self, ApiConfigError> {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    pub fn from_getter<F>(mut get: F) -> Result<Self, ApiConfigError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let raw_bind_addr =
            get(API_BIND_ADDR_ENV).unwrap_or_else(|| DEFAULT_API_BIND_ADDR.to_owned());
        let bind_addr =
            raw_bind_addr
                .parse()
                .map_err(|source| ApiConfigError::InvalidBindAddr {
                    value: raw_bind_addr,
                    source,
                })?;

        let issuer = required_config(&mut get, COGNITO_ISSUER_ENV)?;
        let jwks_url = required_config(&mut get, COGNITO_JWKS_URL_ENV)?;
        let app_client_ids = required_config(&mut get, COGNITO_APP_CLIENT_IDS_ENV)?
            .split(',')
            .map(str::trim)
            .filter(|client_id| !client_id.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if app_client_ids.is_empty() {
            return Err(ApiConfigError::EmptyCognitoAppClientIds);
        }
        let cognito_user_pool_id = required_config(&mut get, COGNITO_USER_POOL_ID_ENV)?;
        let vertex_ai_embedding = VertexAiEmbeddingConfig::new(
            get(VERTEX_AI_PROJECT_ID_ENV)
                .unwrap_or_else(|| DEFAULT_VERTEX_AI_PROJECT_ID.to_owned()),
            get(VERTEX_AI_LOCATION_ENV).unwrap_or_else(|| DEFAULT_VERTEX_AI_LOCATION.to_owned()),
        );
        let stripe_billing = StripeBillingConfig {
            api_key: required_config(&mut get, STRIPE_API_KEY_ENV)?,
            checkout_success_url: required_url_config(&mut get, STRIPE_CHECKOUT_SUCCESS_URL_ENV)?,
            checkout_cancel_url: required_url_config(&mut get, STRIPE_CHECKOUT_CANCEL_URL_ENV)?,
            portal_return_url: required_url_config(&mut get, STRIPE_PORTAL_RETURN_URL_ENV)?,
        };
        let billing_prices = BillingPriceIds {
            pro_monthly: required_config(&mut get, STRIPE_PRO_MONTHLY_PRICE_ID_ENV)?,
            pro_yearly: required_config(&mut get, STRIPE_PRO_YEARLY_PRICE_ID_ENV)?,
            ultimate_monthly: required_config(&mut get, STRIPE_ULTIMATE_MONTHLY_PRICE_ID_ENV)?,
            ultimate_yearly: required_config(&mut get, STRIPE_ULTIMATE_YEARLY_PRICE_ID_ENV)?,
        };

        let product_listing_search_parallel_enrichment_enabled = optional_bool_config(
            &mut get,
            PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV,
            false,
        )?;
        let zoho = ZohoConfig {
            list_key: required_config(&mut get, ZOHO_LIST_KEY_ENV)?,
            client_id: required_config(&mut get, ZOHO_CLIENT_ID_ENV)?,
            client_secret: required_config(&mut get, ZOHO_CLIENT_SECRET_ENV)?,
            refresh_token: required_config(&mut get, ZOHO_REFRESH_TOKEN_ENV)?,
            accounts_url: required_config(&mut get, ZOHO_ACCOUNTS_URL_ENV)?,
            campaigns_url: required_config(&mut get, ZOHO_CAMPAIGNS_URL_ENV)?,
        };

        Ok(Self {
            bind_addr,
            cognito_jwt: CognitoJwtConfig::new(issuer, jwks_url, app_client_ids),
            cognito_user_pool_id,
            vertex_ai_embedding,
            stripe_billing,
            billing_prices,
            zoho,
            product_listing_search_parallel_enrichment_enabled,
        })
    }

    pub const fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub fn cognito_jwt(&self) -> &CognitoJwtConfig {
        &self.cognito_jwt
    }

    pub fn cognito_user_pool_id(&self) -> &str {
        &self.cognito_user_pool_id
    }

    pub fn vertex_ai_embedding(&self) -> &VertexAiEmbeddingConfig {
        &self.vertex_ai_embedding
    }

    fn stripe_billing(&self) -> &StripeBillingConfig {
        &self.stripe_billing
    }

    fn billing_prices(&self) -> &BillingPriceIds {
        &self.billing_prices
    }

    fn zoho(&self) -> &ZohoConfig {
        &self.zoho
    }

    fn product_listing_search_read_execution_policy(
        &self,
    ) -> ProductListingSearchReadExecutionPolicy {
        if self.product_listing_search_parallel_enrichment_enabled {
            ProductListingSearchReadExecutionPolicy::Concurrent
        } else {
            ProductListingSearchReadExecutionPolicy::Sequential
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct ZohoConfig {
    list_key: String,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    accounts_url: String,
    campaigns_url: String,
}

fn required_config<F>(get: &mut F, name: &'static str) -> Result<String, ApiConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    get(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or(ApiConfigError::MissingRequiredConfig { name })
}

fn required_url_config<F>(get: &mut F, name: &'static str) -> Result<url::Url, ApiConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    let value = required_config(get, name)?;
    url::Url::parse(&value).map_err(|source| ApiConfigError::InvalidUrlConfig { name, source })
}

fn optional_bool_config<F>(
    get: &mut F,
    name: &'static str,
    default: bool,
) -> Result<bool, ApiConfigError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    match get(name) {
        Some(value) => value
            .parse()
            .map_err(|_| ApiConfigError::InvalidBooleanConfig { name, value }),
        None => Ok(default),
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ApiConfigError {
    #[error("invalid {env_name}: {value}", env_name = API_BIND_ADDR_ENV)]
    InvalidBindAddr {
        value: String,
        source: AddrParseError,
    },
    #[error("missing required configuration {name}")]
    MissingRequiredConfig { name: &'static str },
    #[error("invalid URL configuration {name}")]
    InvalidUrlConfig {
        name: &'static str,
        #[source]
        source: url::ParseError,
    },
    #[error("{COGNITO_APP_CLIENT_IDS_ENV} must contain at least one client id")]
    EmptyCognitoAppClientIds,
    #[error("invalid boolean configuration {name}: {value}")]
    InvalidBooleanConfig { name: &'static str, value: String },
}

pub fn app(state: AppState) -> Router {
    let health_routes = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .with_state(Arc::clone(&state.readiness));
    let mut routes = health_routes;

    if let Some(products) = state.product_listings {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/product-listings",
                    get(product_listings::search_products::get_products),
                )
                .route(
                    "/api/v1/product-listings/{product_listing_id}",
                    get(product_listings::get_product_by_id::get_product_by_id),
                )
                .route(
                    "/api/v1/product-listings/by-slug/{product_listing_title_slug_id}",
                    get(product_listings::get_product_by_title_slug::get_product_by_title_slug),
                )
                .route(
                    "/api/v1/product-listings/{product_listing_id}/history",
                    get(product_listings::get_product_listing_history::get_product_listing_history_by_id),
                )
                .route(
                    "/api/v1/product-listings/{product_listing_id}/similar",
                    get(product_listings::get_similar_products::get_similar_products_by_id),
                )
                .with_state(products),
        );
    }

    if let Some(webhooks) = state.webhooks {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/webhooks/woocommerce/{listing_source_id}",
                    post(webhooks::post_woocommerce::post_woocommerce),
                )
                .with_state(webhooks),
        );
    }

    if let Some(partner_product_listings) = state.partner_product_listings {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/listing-sources/{listing_source_id}/product-listings",
                    post(partner_product_listings::create_products::create_products)
                        .patch(partner_product_listings::update_products::update_products)
                        .put(partner_product_listings::upsert_products::upsert_products)
                        .delete(partner_product_listings::delete_products::delete_products),
                )
                .with_state(partner_product_listings),
        );
    }

    if let Some(admin_overview) = state.admin_overview {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/admin/overview",
                    get(admin_overview::get_admin_overview),
                )
                .with_state(admin_overview),
        );
    }

    if let Some(listing_sources) = state.listing_sources {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/admin/listing-sources/{listing_source_id}",
                    get(listing_sources::get_listing_source::get_listing_source)
                        .patch(listing_sources::update_listing_source::update_listing_source)
                        .delete(listing_sources::delete_listing_source::delete_listing_source),
                )
                .route(
                    "/api/v1/listing-sources/by-slug/{listing_source_slug_id}",
                    get(listing_sources::get_listing_source_by_slug::get_listing_source_by_slug),
                )
                .route(
                    "/api/v1/me/listing-sources",
                    get(listing_sources::list_my_listing_sources::list_my_listing_sources),
                )
                .route(
                    "/api/v1/admin/listing-sources",
                    get(listing_sources::search_listing_sources::search_listing_sources)
                        .post(listing_sources::create_listing_source::create_listing_source),
                )
                .with_state(listing_sources),
        );
    }

    if let Some(oauth) = state.oauth {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/admin/oauth-clients",
                    get(oauth::list_clients::list_clients)
                        .post(oauth::create_client::create_client),
                )
                .route(
                    "/api/v1/admin/oauth-clients/{client_id}",
                    get(oauth::get_client::get_client)
                        .patch(oauth::update_client::update_client)
                        .delete(oauth::delete_client::delete_client),
                )
                .route("/api/v1/oauth/authorize", get(oauth::authorize::authorize))
                .route("/api/v1/oauth/token", post(oauth::token::token))
                .route(
                    "/api/v1/oauth/tokens/by-third-party-code/{third_party_code}",
                    get(oauth::token_by_third_party_code::token_by_third_party_code),
                )
                .route("/api/v1/oauth/revoke", post(oauth::revoke::revoke))
                .route(
                    "/api/v1/oauth/introspect",
                    post(oauth::introspect::introspect),
                )
                .with_state(oauth),
        );
    }

    if let Some(parties) = state.parties {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/admin/parties",
                    get(parties::search_parties::search_parties)
                        .post(parties::create_party::create_party),
                )
                .route(
                    "/api/v1/admin/parties/{party_id}",
                    get(parties::get_party::get_party)
                        .patch(parties::update_party::update_party)
                        .delete(parties::delete_party::delete_party),
                )
                .with_state(parties),
        );
    }

    if let Some(users) = state.users {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/me/account",
                    get(users::account::get_me).patch(users::account::patch_me),
                )
                .route("/api/v1/me", delete(users::account::delete_me))
                .route(
                    "/api/v1/me/access-tokens",
                    get(users::access_tokens::list_access_tokens)
                        .post(users::access_tokens::post_access_token)
                        .patch(users::access_tokens::patch_access_token),
                )
                .route(
                    "/api/v1/me/access-tokens/{access_token_id}",
                    get(users::access_tokens::get_access_token)
                        .delete(users::access_tokens::delete_access_token),
                )
                .route("/api/v1/admin/users", get(users::admin_users::search_users))
                .route(
                    "/api/v1/admin/users/{user_id}",
                    get(users::admin_users::get_user)
                        .patch(users::admin_users::patch_admin_user)
                        .delete(users::admin_users::delete_admin_user),
                )
                .route(
                    "/api/v1/admin/users/{user_id}/suspension",
                    axum::routing::put(users::suspend_user::suspend_user)
                        .delete(users::unsuspend_user::unsuspend_user),
                )
                .route(
                    "/api/v1/admin/users/{user_id}/sessions/revoke",
                    post(users::revoke_user_sessions::revoke_user_sessions),
                )
                .route(
                    "/api/v1/admin/users/{user_id}/access-tokens",
                    get(users::access_tokens::list_admin_access_tokens)
                        .delete(users::access_tokens::delete_admin_access_tokens),
                )
                .route(
                    "/api/v1/admin/users/{user_id}/access-tokens/{access_token_id}",
                    delete(users::access_tokens::delete_admin_access_token),
                )
                .with_state(users),
        );
    }

    if let Some(watchlist) = state.watchlist {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/me/watchlist",
                    get(watchlist::list::list_watchlist).post(watchlist::create::post_watchlist),
                )
                .route(
                    "/api/v1/me/watchlist/{product_listing_id}",
                    patch(watchlist::update::patch_watchlist)
                        .delete(watchlist::delete::delete_watchlist),
                )
                .with_state(watchlist),
        );
    }

    if let Some(search_filters) = state.search_filters {
        routes = routes.merge(search_filters::router(search_filters));
    }

    if let Some(notifications) = state.notifications {
        routes = routes.merge(notifications::router(notifications));
    }

    if let Some(billing) = state.billing {
        routes = routes.merge(
            Router::new()
                .route("/api/v1/me/billing/checkout", post(billing::checkout))
                .route("/api/v1/me/billing/portal", post(billing::portal))
                .route("/api/v1/me/billing/manage", post(billing::manage))
                .with_state(billing),
        );
    }

    if let Some(newsletter) = state.newsletter {
        routes = routes.merge(
            Router::new()
                .route(
                    "/api/v1/newsletter-subscriptions",
                    axum::routing::put(newsletter::put::put_newsletter_subscription),
                )
                .with_state(newsletter),
        );
    }

    if let Some(partnership_applications) = state.partnership_applications {
        routes = routes.merge(partnership_applications::router(partnership_applications));
    }

    if let Some(partnerships) = state.partnerships {
        routes = routes.merge(partnerships::router(partnerships));
    }

    with_transport_middleware(routes)
}

async fn health() -> &'static str {
    "ok\n"
}

async fn ready(
    axum::extract::State(readiness): axum::extract::State<Arc<dyn ReadinessCheck>>,
) -> axum::http::StatusCode {
    match readiness.check().await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT,
        Err(()) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
    }
}

struct RuntimeReadiness {
    postgres: PgPool,
    opensearch: OpenSearch,
}

use async_trait::async_trait;
use aws_config::BehaviorVersion;

#[async_trait]
impl ReadinessCheck for RuntimeReadiness {
    async fn check(&self) -> Result<(), ()> {
        self.postgres.acquire().await.map_err(|_| ())?;
        self.opensearch.ping().send().await.map_err(|_| ())?;
        Ok(())
    }
}

pub async fn app_state_from_env() -> Result<AppState, ApiStateError> {
    let config = ApiConfig::from_env().map_err(ApiStateError::Config)?;
    app_state_from_config(&config).await
}

async fn app_state_from_config(config: &ApiConfig) -> Result<AppState, ApiStateError> {
    let cognito_config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let cognito_session_revoker = CognitoUserSessionRevoker::new(
        aws_sdk_cognitoidentityprovider::Client::new(&cognito_config),
        config.cognito_user_pool_id(),
    );
    let pool = postgres_pool_from_env().await?;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let get_product_listing_history = GetProductListingHistoryHandler::new(
        unit_of_work.clone(),
        SqlxProductListingHistoryReaderFactory::new(),
    );
    let search_filter_reader = SqlxSearchFilterReader::new(pool.clone());
    let opensearch_client = opensearch_client_from_env()?;
    let embeddings: Arc<dyn EmbeddingGenerator> = Arc::new(VertexAiEmbeddingGenerator::new(
        config.vertex_ai_embedding().clone(),
        google_application_default_credentials()?,
    ));

    let get_admin_overview = GetAdminOverviewHandler::new(
        unit_of_work.clone(),
        SqlxAdminOverviewReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let create_listing_source = CreateListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let get_listing_source = GetListingSourceHandler::new(
        SqlxListingSourceReaders::new(pool.clone()),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let update_listing_source = UpdateListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let delete_listing_source = DeleteListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let list_administered_listing_sources = ListAdministeredListingSourcesHandler::new(
        SqlxListingSourceAuthorization::new(pool.clone()),
    );
    let search_listing_sources = SearchListingSourcesHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceSearchReaderFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let get_own_user =
        GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new());
    let stripe_billing = StripeBillingClient::new(config.stripe_billing().clone())
        .map_err(ApiStateError::StripeBilling)?;
    let billing_prices = config.billing_prices().clone();
    let admin_get_user = AdminGetUserHandler::new(
        unit_of_work.clone(),
        SqlxUserAccountReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let search_users = SearchUsersHandler::new(
        unit_of_work.clone(),
        SqlxUserSearchReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let create_party = CreatePartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let get_party = GetPartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let update_party = UpdatePartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let delete_party = DeletePartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let search_parties = SearchPartiesHandler::new(
        unit_of_work.clone(),
        SqlxPartySearchReaderFactory::new(),
        CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
    );
    let update_user_profile = UpdateUserProfileHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let admin_update_user_profile = UpdateUserProfileHandler::new_admin_only(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let change_user_role = ChangeUserRoleHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let change_user_tier = ChangeUserTierHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
        SqlxUserTierEntitlementsFactory::new(),
    );
    let delete_user = DeleteUserHandler::new(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let admin_delete_user = DeleteUserHandler::new_admin_only(
        unit_of_work.clone(),
        SqlxUserRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let newsletter_writer = ZohoNewsletterSubscriptionWriter::new(
        config.zoho().list_key.clone(),
        reqwest::Client::new(),
        config.zoho().client_id.clone(),
        config.zoho().client_secret.clone(),
        config.zoho().refresh_token.clone(),
        config.zoho().accounts_url.clone(),
        config.zoho().campaigns_url.clone(),
    );
    let upsert_newsletter_subscription = UpsertNewsletterSubscriptionHandler::new(
        SqlxNewsletterProfileReader::new(pool.clone()),
        newsletter_writer,
    );
    let watch_product = WatchProductListingHandler::new(
        unit_of_work.clone(),
        SqlxWatchlistRepositoryFactory,
        SqlxWatchlistQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory::new(),
        SqlxProductListingLifecycleGuardFactory::new(),
    );
    let update_watchlist_product = UpdateWatchlistProductListingHandler::new(
        unit_of_work.clone(),
        SqlxWatchlistRepositoryFactory,
        SqlxWatchlistQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory::new(),
        SqlxProductListingLifecycleGuardFactory::new(),
    );
    let unwatch_product =
        UnwatchProductListingHandler::new(unit_of_work.clone(), SqlxWatchlistRepositoryFactory);
    let submit_partnership_application = SubmitPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
    );
    let list_own_partnership_applications = ListOwnPartnershipApplicationsHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationReaderFactory::new(),
    );
    let get_own_partnership_application = GetOwnPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
    );
    let withdraw_partnership_application = WithdrawPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
    );
    let list_admin_partnership_applications = ListAdminPartnershipApplicationsHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let list_admin_partnerships = ListAdminPartnershipsHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipSearchReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let get_admin_partnership = GetAdminPartnershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipDetailsReaderFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let dissolve_partnership = DissolvePartnershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let grant_partnership_membership = GrantPartnershipMembershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAccountReaderFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let revoke_partnership_membership = RevokePartnershipMembershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAccountReaderFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let grant_partnership_listing_source = GrantPartnershipListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let revoke_partnership_listing_source = RevokePartnershipListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let get_partnership_application = GetPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let mark_partnership_application_in_review = MarkPartnershipApplicationInReviewHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
    );
    let approve_partnership_application = ApprovePartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxPartyRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
    );
    let reject_partnership_application = RejectPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxPartyRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxUserAdminReaderFactory::new(),
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
    );
    let product_user_states = SqlxProductListingUserStateReader::new(pool.clone());
    let get_similar_products = GetSimilarProductListingsHandler::new(
        unit_of_work.clone(),
        SqlxProductListingEmbeddingReaderFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
        OpenSearchProductListingSimilarProductListingsReader::new(opensearch_client.clone()),
        SqlxListingSourceSummaryReader::new(pool.clone()),
        product_user_states.clone(),
        SqlxProductListingContentAssessmentReader::new(pool.clone()),
    );
    let search_products = SearchProductListingsHandler::new(
        unit_of_work.clone(),
        OpenSearchProductListingSearchReader::new(opensearch_client.clone()),
        SqlxFxRateSnapshotRepositoryFactory,
        Arc::clone(&embeddings),
        SqlxListingSourceSummaryReader::new(pool.clone()),
        product_user_states,
        SqlxProductListingContentAssessmentReader::new(pool.clone()),
    )
    .with_read_execution_policy(config.product_listing_search_read_execution_policy());
    let get_product = GetProductListingHandler::new(
        unit_of_work.clone(),
        SqlxProductListingDetailsReaderFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
    );
    let create_product = CreateProductListingHandler::new(
        unit_of_work.clone(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let update_product = UpdateProductListingHandler::new(
        unit_of_work.clone(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let upsert_product = UpsertProductListingHandler::new(
        unit_of_work.clone(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let withdraw_product = WithdrawProductListingHandler::new(
        unit_of_work.clone(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let capture_woocommerce_product = CaptureProductListingRawObservationHandler::new(
        unit_of_work.clone(),
        SqlxProductListingRawCaptureWriterFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let authorize_woocommerce_product = AuthorizeProductListingRawCaptureHandler::new(
        unit_of_work.clone(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
    );
    let intake_woocommerce_product = WoocommerceWebhookIntake::new(
        SqlxListingSourceReaders::new(pool.clone()),
        SqlxListingSourceReaders::new(pool.clone()),
        capture_woocommerce_product,
        authorize_woocommerce_product,
    );
    let list_watchlist = ListWatchlistHandler::new(
        unit_of_work.clone(),
        SqlxProductListingWatchlistDetailsReaderFactory::new(),
        SqlxFxRateSnapshotRepositoryFactory,
    );

    let access_token_use_case =
        AuthenticateAccessTokenHandler::new(SqlxAccessTokenAuthenticationReader::new(pool.clone()));
    let resolve_cognito_user =
        ResolveCognitoUserHandler::new(SqlxCognitoUserIdentityReader::new(pool.clone()));
    let authenticate_user =
        AuthenticateUserHandler::new(SqlxUserAuthenticationReader::new(pool.clone()));
    let jwks_client = reqwest::Client::builder()
        .connect_timeout(JWKS_CONNECT_TIMEOUT)
        .timeout(JWKS_REQUEST_TIMEOUT)
        .build()
        .map_err(ApiStateError::JwksClient)?;
    let authenticator = compose_authenticator(
        config,
        ReqwestJwksProvider::new(jwks_client),
        resolve_cognito_user,
        AuraAccessTokenAuthenticator::new(access_token_use_case),
        authenticate_user,
    )
    .map_err(ApiStateError::CognitoJwt)?;
    let notifications_state = NotificationsState::new(
        Arc::new(ListNotificationsHandler::new(
            SqlxNotificationListReader::new(pool.clone()),
        )),
        Arc::new(UpdateNotificationSeenHandler::new(
            SqlxNotificationSeenWriter::new(pool.clone()),
        )),
        Arc::new(UpdateNotificationsSeenHandler::new(
            SqlxNotificationSeenWriter::new(pool.clone()),
        )),
        Arc::new(UpdateAllNotificationsSeenHandler::new(
            SqlxNotificationSeenWriter::new(pool.clone()),
        )),
        Arc::new(DeleteNotificationHandler::new(
            SqlxNotificationDeleter::new(pool.clone()),
        )),
        Arc::new(DeleteNotificationsHandler::new(
            SqlxNotificationDeleter::new(pool.clone()),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let billing_state = BillingState::new(
        Arc::new(CreateBillingCheckoutSessionHandler::new(
            GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new()),
            AssociateUserStripeCustomerIdHandler::new(
                unit_of_work.clone(),
                SqlxUserRepositoryFactory::new(),
            ),
            stripe_billing.clone(),
            stripe_billing.clone(),
            billing_prices.clone(),
        )),
        Arc::new(CreateBillingPortalSessionHandler::new(
            GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new()),
            stripe_billing.clone(),
        )),
        Arc::new(CreateBillingManagementSessionHandler::new(
            GetOwnUserHandler::new(unit_of_work.clone(), SqlxUserAccountReaderFactory::new()),
            AssociateUserStripeCustomerIdHandler::new(
                unit_of_work.clone(),
                SqlxUserRepositoryFactory::new(),
            ),
            stripe_billing.clone(),
            stripe_billing.clone(),
            stripe_billing,
            billing_prices,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let partner_product_listings_state = PartnerProductListingsState::new(
        Arc::new(create_product),
        Arc::new(update_product),
        Arc::new(upsert_product),
        Arc::new(withdraw_product),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let listing_sources_state = ListingSourcesState::new(
        Arc::new(create_listing_source),
        Arc::new(get_listing_source),
        Arc::new(update_listing_source),
        Arc::new(list_administered_listing_sources),
        Arc::new(search_listing_sources),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    )
    .with_delete(Arc::new(delete_listing_source));
    let parties_state = PartiesState::new(
        Arc::new(create_party),
        Arc::new(get_party),
        Arc::new(search_parties),
        Arc::new(update_party),
        Arc::new(delete_party),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let users_state = UsersState {
        get_own_user: Arc::new(get_own_user),
        admin_get_user: Arc::new(admin_get_user),
        search_users: Arc::new(search_users),
        update_user_profile: Arc::new(update_user_profile),
        admin_update_user_profile: Arc::new(admin_update_user_profile),
        change_user_role: Arc::new(change_user_role),
        change_user_tier: Arc::new(change_user_tier),
        delete_user: Arc::new(delete_user),
        admin_delete_user: Arc::new(admin_delete_user),
        suspend_user: Arc::new(SuspendUserHandler::new(
            unit_of_work.clone(),
            SqlxUserRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
        )),
        unsuspend_user: Arc::new(UnsuspendUserHandler::new(
            unit_of_work.clone(),
            SqlxUserRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
        )),
        revoke_user_sessions: Arc::new(RevokeUserSessionsHandler::new(
            unit_of_work.clone(),
            SqlxUserAdminReaderFactory::new(),
            SqlxUserCognitoIdentityRegistryFactory::new(),
            cognito_session_revoker,
        )),
        create_access_token: Arc::new(CreateAccessTokenHandler::new(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
        )),
        list_access_tokens: Arc::new(ListAccessTokensHandler::new(
            SqlxAccessTokenListReader::new(pool.clone()),
        )),
        admin_list_access_tokens: Arc::new(ListAdminAccessTokensHandler::new(
            unit_of_work.clone(),
            SqlxAdminAccessTokenListReaderFactory::new(),
            SqlxUserAdminReaderFactory::new(),
            SqlxUserAccountReaderFactory::new(),
        )),
        get_access_token: Arc::new(GetAccessTokenHandler::new(
            SqlxAccessTokenDetailsReader::new(pool.clone()),
        )),
        update_access_token: Arc::new(UpdateAccessTokenHandler::new(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
        )),
        delete_access_token: Arc::new(DeleteAccessTokenHandler::new(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
        )),
        admin_delete_access_token: Arc::new(DeleteAccessTokenHandler::new_admin_only(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
        )),
        admin_delete_access_tokens: Arc::new(DeleteAccessTokensHandler::new(
            unit_of_work.clone(),
            SqlxAccessTokenRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
            SqlxUserAccountReaderFactory::new(),
        )),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let search_filters_state = SearchFiltersState::new(
        Arc::new(ListOwnedSearchFiltersHandler::new(
            search_filter_reader.clone(),
        )),
        Arc::new(CreateSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            Arc::clone(&embeddings),
            SqlxSearchFilterQuotaReaderFactory,
            SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(GetOwnedSearchFilterHandler::new(
            search_filter_reader.clone(),
        )),
        Arc::new(UpdateOwnedSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            Arc::clone(&embeddings),
            search_filter_reader.clone(),
            SqlxSearchFilterQuotaReaderFactory,
            SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(DeleteOwnedSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
        )),
        Arc::new(ListSearchFilterMatchesHandler::new(
            unit_of_work.clone(),
            search_filter_reader.clone(),
            SqlxProductListingDetailsBatchReader::new(pool.clone()),
            SqlxFxRateSnapshotRepositoryFactory,
        )),
        Arc::new(UpdateSearchFilterMatchFeedbackHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            SqlxSearchFilterMatchRepositoryFactory,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let watchlist_state = WatchlistState {
        list_watchlist: Arc::new(list_watchlist),
        watch_product: Arc::new(watch_product),
        update_watchlist_product: Arc::new(update_watchlist_product),
        unwatch_product: Arc::new(unwatch_product),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };
    let oauth_state = OAuthState {
        create_client: Arc::new(CreateOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        list_clients: Arc::new(ListOAuthClientsHandler::new(
            SqlxOAuthClientListReader::new(pool.clone()),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        get_client: Arc::new(GetOAuthClientHandler::new(
            SqlxOAuthClientDetailsReader::new(pool.clone()),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        update_client: Arc::new(UpdateOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        delete_client: Arc::new(DeleteOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(unit_of_work.clone(), SqlxUserAdminReaderFactory::new()),
        )),
        authorize: Arc::new(AuthorizeHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            SqlxAuthorizationCodeRepositoryFactory::new(),
        )),
        token_by_authorization_code: Arc::new(TokenByAuthorizationCodeHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            SqlxAuthorizationCodeRepositoryFactory::new(),
            SqlxThirdPartyExchangeCodeRepositoryFactory::new(),
            SqlxAccessTokenRepositoryFactory::new(),
        )),
        token_by_third_party_code: Arc::new(TokenByThirdPartyCodeHandler::new(
            unit_of_work.clone(),
            SqlxThirdPartyExchangeCodeRepositoryFactory::new(),
        )),
        revoke: Arc::new(RevokeTokenHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            SqlxAccessTokenRepositoryFactory::new(),
        )),
        introspect: Arc::new(IntrospectTokenHandler::new(
            SqlxOAuthClientAuthenticationReader::new(pool.clone()),
            SqlxAccessTokenAuthenticationReader::new(pool.clone()),
        )),
        authenticator: Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    };

    let partnership_state = PartnershipApplicationsState::new(
        Arc::new(submit_partnership_application),
        Arc::new(list_own_partnership_applications),
        Arc::new(get_own_partnership_application),
        Arc::new(withdraw_partnership_application),
        Arc::new(list_admin_partnership_applications),
        Arc::new(get_partnership_application),
        Arc::new(mark_partnership_application_in_review),
        Arc::new(approve_partnership_application),
        Arc::new(reject_partnership_application),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let partnerships_state = PartnershipsState::new(
        Arc::new(list_admin_partnerships),
        Arc::new(get_admin_partnership),
        Arc::new(grant_partnership_membership),
        Arc::new(revoke_partnership_membership),
        Arc::new(grant_partnership_listing_source),
        Arc::new(revoke_partnership_listing_source),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    )
    .with_dissolve(Arc::new(dissolve_partnership));

    let readiness = Arc::new(RuntimeReadiness {
        postgres: pool,
        opensearch: opensearch_client.clone(),
    });

    Ok(AppState::new()
        .with_admin_overview(AdminOverviewState::new(
            Arc::new(get_admin_overview),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_parties(parties_state)
        .with_users(users_state)
        .with_watchlist(watchlist_state)
        .with_partnership_applications(partnership_state)
        .with_partnerships(partnerships_state)
        .with_products(
            ProductListingsState::new(
                Arc::new(get_product),
                Arc::new(get_similar_products),
                Arc::new(search_products),
                Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
            )
            .with_product_listing_history(Arc::new(get_product_listing_history)),
        )
        .with_partner_product_listings(partner_product_listings_state)
        .with_listing_sources(listing_sources_state)
        .with_webhooks(WebhooksState::new(
            Arc::new(intake_woocommerce_product),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_oauth(oauth_state)
        .with_search_filters(search_filters_state)
        .with_notifications(notifications_state)
        .with_billing(billing_state)
        .with_newsletter(NewsletterState::new(
            Arc::new(upsert_newsletter_subscription),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_readiness(readiness))
}

async fn postgres_pool_from_env() -> Result<PgPool, ApiStateError> {
    let host = required_postgres_env(POSTGRES_HOST_ENV)?;
    let database = required_postgres_env(POSTGRES_DATABASE_ENV)?;
    let username = required_postgres_env(POSTGRES_USERNAME_ENV)?;
    let password = required_postgres_env(POSTGRES_PASSWORD_ENV)?;
    let port = optional_postgres_env(POSTGRES_PORT_ENV, DEFAULT_POSTGRES_PORT)?;
    let max_connections = optional_postgres_env(
        POSTGRES_MAX_CONNECTIONS_ENV,
        DEFAULT_POSTGRES_MAX_CONNECTIONS,
    )?;
    let config = PostgresPoolConfig::new(host, port, database, username, password, max_connections)
        .map_err(|_| ApiStateError::InvalidPostgresMaxConnections)?;

    Ok(config
        .connect()
        .await
        .map_err(PostgresConnectError::Connect)?)
}

fn required_postgres_env(name: &'static str) -> Result<String, ApiStateError> {
    std::env::var(name).map_err(|_| ApiStateError::MissingEnv { name })
}

fn optional_postgres_env<T>(name: &'static str, default: T) -> Result<T, ApiStateError>
where
    T: std::str::FromStr,
{
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| ApiStateError::InvalidPostgresInteger { name, value }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(ApiStateError::MissingEnv { name }),
    }
}

fn opensearch_client_from_env() -> Result<OpenSearch, ApiStateError> {
    let endpoint =
        std::env::var("OPENSEARCH_ENDPOINT_URL").map_err(|_| ApiStateError::MissingEnv {
            name: "OPENSEARCH_ENDPOINT_URL",
        })?;
    let endpoint = url::Url::parse(&endpoint).map_err(|error| ApiStateError::OpenSearch {
        detail: error.to_string(),
    })?;
    let stage = std::env::var("STAGE").unwrap_or_else(|_| "prod".to_owned());
    let transport = if stage == "ephemeral" {
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint)).build()
    } else {
        let username =
            std::env::var("OPENSEARCH_USERNAME").map_err(|_| ApiStateError::MissingEnv {
                name: "OPENSEARCH_USERNAME",
            })?;
        let password =
            std::env::var("OPENSEARCH_PASSWORD").map_err(|_| ApiStateError::MissingEnv {
                name: "OPENSEARCH_PASSWORD",
            })?;
        TransportBuilder::new(SingleNodeConnectionPool::new(endpoint))
            .auth(Credentials::Basic(username, password))
            .build()
    }
    .map_err(|error| ApiStateError::OpenSearch {
        detail: error.to_string(),
    })?;
    Ok(OpenSearch::new(transport))
}

fn google_application_default_credentials()
-> Result<google_cloud_auth::credentials::AccessTokenCredentials, ApiStateError> {
    GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|error| ApiStateError::VertexAiCredentials {
            detail: error.to_string(),
        })
}

fn compose_authenticator<P, R, A, U>(
    config: &ApiConfig,
    jwks_provider: P,
    resolve_cognito_user: R,
    access_token_authenticator: A,
    authenticate_user: U,
) -> Result<Arc<dyn TokenAuthenticator>, AuthError>
where
    P: JwksProvider + 'static,
    R: user_service::use_cases::ResolveCognitoUserUseCase + 'static,
    A: TokenAuthenticator + 'static,
    U: user_service::use_cases::AuthenticateUserUseCase + 'static,
{
    let cognito_jwt = CognitoJwtAuthenticator::new(
        config.cognito_jwt().clone(),
        jwks_provider,
        resolve_cognito_user,
    )?;
    Ok(Arc::new(UserAuthenticationAuthenticator::new(
        ApiAuthService::new(cognito_jwt, access_token_authenticator),
        authenticate_user,
    )))
}

#[derive(thiserror::Error, Debug)]
pub enum ApiStateError {
    #[error("failed to initialize Stripe billing client")]
    StripeBilling(#[source] billing_stripe::StripeBillingClientInitError),
    #[error(transparent)]
    Postgres(#[from] PostgresConnectError),
    #[error(transparent)]
    Config(#[from] ApiConfigError),
    #[error("invalid integer in environment variable {name}: {value}")]
    InvalidPostgresInteger { name: &'static str, value: String },
    #[error("POSTGRES_MAX_CONNECTIONS must be greater than zero")]
    InvalidPostgresMaxConnections,
    #[error("missing required environment variable {name}")]
    MissingEnv { name: &'static str },
    #[error("failed to configure OpenSearch: {detail}")]
    OpenSearch { detail: String },
    #[error("failed to initialize Vertex AI credentials: {detail}")]
    VertexAiCredentials { detail: String },
    #[error("failed to configure Cognito JWT authentication: {0}")]
    CognitoJwt(AuthError),
    #[error("failed to build JWKS HTTP client: {0}")]
    JwksClient(reqwest::Error),
}

pub async fn run_until_shutdown<S>(config: ApiConfig, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let state = app_state_from_config(&config)
        .await
        .map_err(ApiRunError::State)?;
    let listener = TcpListener::bind(config.bind_addr())
        .await
        .map_err(ApiRunError::Bind)?;
    serve(listener, app(state), shutdown).await
}

pub async fn serve<S>(listener: TcpListener, app: Router, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let local_addr = listener.local_addr().map_err(ApiRunError::LocalAddr)?;
    info!(bind_addr = %local_addr, "aura-historia-api listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(ApiRunError::Serve)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_default_parallel_product_listing_enrichment_to_disabled() {
        let mut get = |_| None;

        let enabled = optional_bool_config(
            &mut get,
            PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV,
            false,
        );

        assert!(matches!(enabled, Ok(false)));
    }

    #[test]
    fn should_reject_non_boolean_parallel_product_listing_enrichment_configuration() {
        let mut get = |name| {
            (name == PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV)
                .then(|| "enabled".to_owned())
        };

        let result = optional_bool_config(
            &mut get,
            PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV,
            false,
        );

        assert!(matches!(
            result,
            Err(ApiConfigError::InvalidBooleanConfig { name, value })
                if name == PRODUCT_LISTING_SEARCH_PARALLEL_ENRICHMENT_ENABLED_ENV && value == "enabled"
        ));
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ApiRunError {
    #[error("failed to build API state")]
    State(#[source] ApiStateError),
    #[error("failed to bind API listener")]
    Bind(#[source] std::io::Error),
    #[error("failed to read API listener local address")]
    LocalAddr(#[source] std::io::Error),
    #[error("failed to serve API")]
    Serve(#[source] std::io::Error),
}

#![allow(dead_code)]

use admin_overview_postgres::SqlxAdminOverviewReaderFactory;
use admin_overview_service::GetAdminOverviewHandler;
use application::transaction::{Transaction, UnitOfWork};
use aura_historia_api::auth::{
    ApiAuthService, AuraAccessTokenAuthenticator, AuthError, RequestMetadata, TokenAuthenticator,
    TransportPrincipal,
};
use aura_historia_api::state::{
    AdminOverviewState, AppState, BillingState, ListingSourcesState, NewsletterState,
    NotificationsState, OAuthState, PartiesState, PartnerProductListingsState,
    PartnershipApplicationsState, PartnershipsState, ProductListingsState, SearchFiltersState,
    UsersState, WatchlistState, WebhooksState,
};
use aura_historia_api::{app, state};
use billing_service::ports::{
    CreateStripeCheckoutSessionRequest, CreateStripeCustomerRequest,
    CreateStripePortalSessionRequest, StripeBillingError, StripeCheckoutSessionCreator,
    StripeCustomerCreator, StripePortalSessionCreator,
};
use billing_service::use_cases::{
    BillingPriceIds, CreateBillingCheckoutSessionHandler, CreateBillingManagementSessionHandler,
    CreateBillingPortalSessionHandler,
};
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
};
use fxrate_core::FxRateId;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use listing_source_postgres::{
    SqlxListingSourceReaders, SqlxListingSourceRepositoryFactory,
    SqlxListingSourceSearchReaderFactory,
};
use listing_source_service::use_cases::commands::create_listing_source::CreateListingSourceHandler;
use listing_source_service::use_cases::commands::update_listing_source::UpdateListingSourceHandler;
use listing_source_service::use_cases::queries::get_listing_source::GetListingSourceHandler;
use listing_source_service::use_cases::queries::search_listing_sources::SearchListingSourcesHandler;
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
use partnership_core::{
    partnership_application_id::PartnershipApplicationId, partnership_id::PartnershipId,
};
use partnership_postgres::{
    SqlxListingSourceAuthorization, SqlxListingSourceGrantRepositoryFactory,
    SqlxPartnershipApplicationReaderFactory, SqlxPartnershipApplicationRepositoryFactory,
    SqlxPartnershipDetailsReaderFactory, SqlxPartnershipRepositoryFactory,
    SqlxPartnershipSearchReaderFactory,
};
use partnership_service::use_cases::{
    commands::{
        approve_partnership_application::ApprovePartnershipApplicationHandler,
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
use party_core::party_id::PartyId;
use party_postgres::{SqlxPartyRepositoryFactory, SqlxPartySearchReaderFactory};
use party_service::use_cases::commands::create_party::CreatePartyHandler;
use party_service::use_cases::commands::update_party::UpdatePartyHandler;
use party_service::use_cases::queries::get_party::GetPartyHandler;
use party_service::use_cases::queries::search_parties::SearchPartiesHandler;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_opensearch::{
    OpenSearchProductListingSearchReader, OpenSearchProductListingSimilarProductListingsReader,
};
use product_listing_postgres::{
    SqlxListingSourceSummaryReader, SqlxPartnerProductListingAuthorizerFactory,
    SqlxProductListingContentAssessmentReader, SqlxProductListingDetailsBatchReader,
    SqlxProductListingDetailsReaderFactory, SqlxProductListingEmbeddingReaderFactory,
    SqlxProductListingEventAppenderFactory, SqlxProductListingHistoryReaderFactory,
    SqlxProductListingRepositoryFactory, SqlxProductListingUserStateReader,
    SqlxProductListingWatchlistDetailsReaderFactory,
};
use user_core::stripe_customer_id::StripeCustomerId;
use user_core::user_id::UserId;

use product_listing_service::use_cases::{
    CreateProductListingHandler, GetProductListingHandler, GetProductListingHistoryHandler,
    GetSimilarProductListingsHandler, IngestWoocommerceProductListingHandler,
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
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use test_api::{get_opensearch_client, get_postgres_client};
use time::OffsetDateTime;
use url::Url;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    Scope,
};
use user_core::tier::UserTier;
use user_service::ports::{
    AccessTokenRepository, AccessTokenRepositoryFactory, NewsletterSubscriptionWriteError,
    NewsletterSubscriptionWriter,
};
use user_service::use_cases::commands::associate_user_stripe_customer_id::AssociateUserStripeCustomerIdHandler;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleHandler;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierHandler;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenHandler;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenHandler;
use user_service::use_cases::commands::delete_user::DeleteUserHandler;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenHandler;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileHandler;
use user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionHandler;
use user_service::use_cases::queries::admin_get_user::AdminGetUserHandler;
use user_service::use_cases::queries::check_user_admin::CheckUserAdminHandler;
use user_service::use_cases::queries::get_access_token::GetAccessTokenHandler;
use user_service::use_cases::queries::get_own_user::GetOwnUserHandler;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensHandler;
use user_service::use_cases::queries::search_users::SearchUsersHandler;
use watchlist_postgres::{SqlxWatchlistQuotaReaderFactory, SqlxWatchlistRepositoryFactory};
use watchlist_service::use_cases::{
    ListWatchlistHandler, UnwatchProductListingHandler, UpdateWatchlistProductListingHandler,
    WatchProductListingHandler,
};

#[derive(Clone, Copy)]
enum TestEmbeddingGenerator {
    Success,
    Failure,
}

impl TestEmbeddingGenerator {
    fn embedding(&self) -> Result<EmbeddingVector, EmbeddingError> {
        match self {
            Self::Success => EmbeddingVector::try_new(vec![1.0; embedding::EMBEDDING_DIMENSIONS]),
            Self::Failure => Err(EmbeddingError::InvalidInput {
                reason: "test embedding failure",
            }),
        }
    }
}

#[async_trait::async_trait]
impl EmbeddingGenerator for TestEmbeddingGenerator {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.embedding()
    }

    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        self.embedding()
    }
}

#[derive(Clone, Copy)]
struct TestStripeBilling;

#[async_trait::async_trait]
impl StripeCustomerCreator for TestStripeBilling {
    async fn create_customer(
        &self,
        request: CreateStripeCustomerRequest,
    ) -> Result<StripeCustomerId, StripeBillingError> {
        Ok(StripeCustomerId::from(format!("cus_{}", request.user_id)))
    }
}

#[async_trait::async_trait]
impl StripeCheckoutSessionCreator for TestStripeBilling {
    async fn create_checkout_session(
        &self,
        request: CreateStripeCheckoutSessionRequest,
    ) -> Result<Url, StripeBillingError> {
        Ok(url(&format!(
            "https://checkout.stripe.test/{}/{}",
            request.stripe_customer_id, request.price_id
        )))
    }
}

#[async_trait::async_trait]
impl StripePortalSessionCreator for TestStripeBilling {
    async fn create_portal_session(
        &self,
        request: CreateStripePortalSessionRequest,
    ) -> Result<Url, StripeBillingError> {
        Ok(url(&format!(
            "https://billing.stripe.test/{}",
            request.stripe_customer_id
        )))
    }
}

#[derive(Clone, Copy)]
struct SuccessfulNewsletterWriter;

#[async_trait::async_trait]
impl NewsletterSubscriptionWriter for SuccessfulNewsletterWriter {
    async fn upsert(
        &self,
        _subscription: &user_core::newsletter_subscription::NewsletterSubscription,
    ) -> Result<(), NewsletterSubscriptionWriteError> {
        Ok(())
    }
}

pub fn aura_api_app() -> Pin<Box<dyn Future<Output = axum::Router> + Send>> {
    Box::pin(async { app(test_state(TestEmbeddingGenerator::Success).await) })
}

pub fn aura_api_app_with_failed_search_embedding()
-> Pin<Box<dyn Future<Output = axum::Router> + Send>> {
    Box::pin(async { app(test_state(TestEmbeddingGenerator::Failure).await) })
}

pub async fn json_response(
    response: reqwest::Response,
) -> (reqwest::StatusCode, serde_json::Value) {
    let status = response.status();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|error| panic!("failed to decode API response: {error}"));
    (status, body)
}

pub fn assert_problem(
    status: reqwest::StatusCode,
    body: &serde_json::Value,
    expected_status: reqwest::StatusCode,
    expected_error: &str,
) {
    assert_eq!(expected_status, status);
    assert_eq!(
        serde_json::json!(u16::from(expected_status)),
        body["status"]
    );
    assert_eq!(serde_json::json!(expected_error), body["error"]);
}

pub async fn seed_user(role: &'static str) -> UserId {
    seed_user_with_tier_and_consent(role, UserTier::Free, false).await
}

pub async fn seed_party(name: &str, phone: Option<&str>, email: Option<&str>) -> PartyId {
    let party_id = PartyId::new();
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        "INSERT INTO parties (party_id, party_slug_id, name, phone, email) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(uuid::Uuid::from(party_id))
    .bind(format!("api-acceptance-party-{party_id}"))
    .bind(name)
    .bind(phone)
    .bind(email)
    .execute(&pool)
    .await
    {
        panic!("failed to seed party: {error}");
    }
    party_id
}

pub async fn seed_partnership_application(
    applicant_user_id: UserId,
    state: &str,
    proposal: serde_json::Value,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> PartnershipApplicationId {
    let application_id = PartnershipApplicationId::new();
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal, created, updated) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(uuid::Uuid::from(application_id))
    .bind(uuid::Uuid::from(applicant_user_id))
    .bind(state)
    .bind(proposal)
    .bind(created)
    .bind(updated)
    .execute(&pool)
    .await
    {
        panic!("failed to seed partnership application: {error}");
    }
    application_id
}

pub async fn seed_partnership_for_search(
    party_name: &str,
    created: OffsetDateTime,
    updated: OffsetDateTime,
    member_user_ids: &[UserId],
    listing_source_ids: &[uuid::Uuid],
) -> (PartnershipId, PartyId) {
    let party_id = PartyId::new();
    let partnership_id = PartnershipId::new();
    let pool = get_postgres_client().await;
    let mut transaction = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin partnership seed transaction: {error}"));

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(uuid::Uuid::from(party_id))
        .bind(format!("api-partnership-party-{party_id}"))
        .bind(party_name)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed partnership party: {error}"));
    sqlx::query(
        "INSERT INTO partnerships (partnership_id, party_id, created, updated) VALUES ($1, $2, $3, $4)",
    )
    .bind(uuid::Uuid::from(partnership_id))
    .bind(uuid::Uuid::from(party_id))
    .bind(created)
    .bind(updated)
    .execute(&mut *transaction)
    .await
    .unwrap_or_else(|error| panic!("failed to seed partnership: {error}"));

    for user_id in member_user_ids {
        sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
            .bind(uuid::Uuid::from(*user_id))
            .bind(uuid::Uuid::from(partnership_id))
            .execute(&mut *transaction)
            .await
            .unwrap_or_else(|error| panic!("failed to seed partnership member: {error}"));
    }
    for listing_source_id in listing_source_ids {
        sqlx::query(
            "INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)",
        )
        .bind(uuid::Uuid::from(partnership_id))
        .bind(*listing_source_id)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed partnership listing-source grant: {error}"));
    }

    transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit partnership seed transaction: {error}"));
    (partnership_id, party_id)
}

pub async fn seed_approved_partnership_application(
    applicant_user_id: UserId,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) -> (PartnershipApplicationId, uuid::Uuid, uuid::Uuid) {
    let approved_listing_source_id = seed_listing_source().await;
    let pool = get_postgres_client().await;
    let approved_partnership_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT partnership.partnership_id FROM partnerships AS partnership JOIN listing_sources AS source ON source.operator_party_id = partnership.party_id WHERE source.listing_source_id = $1 ORDER BY partnership.partnership_id LIMIT 1",
    )
    .bind(approved_listing_source_id)
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to find seeded partnership: {error}"));
    let application_id = PartnershipApplicationId::new();
    if let Err(error) = sqlx::query(
        "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal, approved_partnership_id, approved_listing_source_id, created, updated) VALUES ($1, $2, 'APPROVED', $3, $4, $5, $6, $7)",
    )
    .bind(uuid::Uuid::from(application_id))
    .bind(uuid::Uuid::from(applicant_user_id))
    .bind(serde_json::json!({
        "type": "EXISTING_LISTING_SOURCE",
        "listing_source_id": approved_listing_source_id,
    }))
    .bind(approved_partnership_id)
    .bind(approved_listing_source_id)
    .bind(created)
    .bind(updated)
    .execute(&pool)
    .await
    {
        panic!("failed to seed approved partnership application: {error}");
    }
    (
        application_id,
        approved_partnership_id,
        approved_listing_source_id,
    )
}

pub async fn seed_user_with_consent(
    role: &'static str,
    show_unassessed_or_sensitive_content: bool,
) -> UserId {
    seed_user_with_tier_and_consent(role, UserTier::Free, show_unassessed_or_sensitive_content)
        .await
}

pub async fn seed_user_with_tier(role: &'static str, tier: UserTier) -> UserId {
    seed_user_with_tier_and_consent(role, tier, false).await
}

async fn seed_user_with_tier_and_consent(
    role: &'static str,
    tier: UserTier,
    show_unassessed_or_sensitive_content: bool,
) -> UserId {
    let user_id = UserId::new();
    let email = format!("{}@example.test", user_id);
    let tier = match tier {
        UserTier::Free => "FREE",
        UserTier::Pro => "PRO",
        UserTier::Ultimate => "ULTIMATE",
    };
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO users (user_id, email, show_unassessed_or_sensitive_content, tier, role)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(email)
    .bind(show_unassessed_or_sensitive_content)
    .bind(tier)
    .bind(role)
    .execute(&pool)
    .await
    {
        panic!("failed to seed user: {error}");
    }
    user_id
}

pub async fn set_user_search_fields(
    user_id: UserId,
    email: &str,
    first_name: &str,
    last_name: &str,
    created: OffsetDateTime,
    updated: OffsetDateTime,
) {
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        "UPDATE users SET email = $2, first_name = $3, last_name = $4, created = $5, updated = $6 WHERE user_id = $1",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(email)
    .bind(first_name)
    .bind(last_name)
    .bind(created)
    .bind(updated)
    .execute(&pool)
    .await
    {
        panic!("failed to set user search fields: {error}");
    }
}

pub async fn set_user_stripe_customer_id(user_id: UserId, stripe_customer_id: &str) {
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query("UPDATE users SET stripe_customer_id = $2 WHERE user_id = $1")
        .bind(uuid::Uuid::from(user_id))
        .bind(stripe_customer_id)
        .execute(&pool)
        .await
    {
        panic!("failed to set user Stripe customer ID: {error}");
    }
}

pub async fn seed_active_watchlist_entries(user_id: UserId, count: usize) {
    for _ in 0..count {
        let product_listing_id = seed_product().await;
        seed_watchlist_entry(user_id, product_listing_id, "ACTIVE").await;
    }
}

pub async fn seed_inactive_watchlist_entry(user_id: UserId) -> ProductListingId {
    let product_listing_id = seed_product().await;
    seed_watchlist_entry(user_id, product_listing_id, "INACTIVE_BY_USER").await;
    product_listing_id
}

async fn seed_watchlist_entry(
    user_id: UserId,
    product_listing_id: ProductListingId,
    state: &'static str,
) {
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        "INSERT INTO product_listing_watchlist (user_id, product_listing_id, notifications, state, active_since, notifications_enabled_since) VALUES ($1, $2, true, $3, CASE WHEN $3 = 'ACTIVE' THEN now() ELSE NULL END, now())",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(state)
    .execute(&pool)
    .await
    {
        panic!("failed to seed watchlist entry: {error}");
    }
}

pub async fn seed_partnership_membership(user_id: UserId, listing_source_id: uuid::Uuid) {
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO partnership_members (user_id, partnership_id)
        SELECT $1, partnership.partnership_id
        FROM listing_sources source
        JOIN partnerships partnership ON partnership.party_id = source.operator_party_id
        WHERE source.listing_source_id = $2
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(listing_source_id)
    .execute(&pool)
    .await
    {
        panic!("failed to seed partnership membership: {error}");
    }
}

pub async fn seed_operator_partnership_listing_source_grant(listing_source_id: uuid::Uuid) {
    let pool = get_postgres_client().await;
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id)
        SELECT partnership.partnership_id, source.listing_source_id
        FROM listing_sources source
        JOIN partnerships partnership ON partnership.party_id = source.operator_party_id
        WHERE source.listing_source_id = $1
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(listing_source_id)
    .execute(&pool)
    .await
    {
        panic!("failed to seed operator listing-source grant: {error}");
    }
}

pub async fn seed_access_token_for(user_id: UserId, scopes: HashSet<Scope>) -> RawAccessToken {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let repository_factory = user_postgres::SqlxAccessTokenRepositoryFactory::new();
    let raw = RawAccessToken::new();
    let token = AccessToken::create(NewAccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw.clone().into(),
        user_id,
        name: AccessTokenName::from("api acceptance"),
        scopes,
        origin: AccessTokenOrigin::User,
        expires: None,
    });
    let mut tx = match unit_of_work.begin().await {
        Ok(tx) => tx,
        Err(error) => panic!("failed to begin access-token seed transaction: {error}"),
    };
    if let Err(error) = repository_factory
        .in_transaction(&mut tx)
        .insert(&token)
        .await
    {
        panic!("failed to seed access token: {error:?}");
    }
    if let Err(error) = tx.commit().await {
        panic!("failed to commit access-token seed transaction: {error}");
    }
    raw
}

pub async fn seed_listing_source() -> uuid::Uuid {
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = uuid::Uuid::new_v4();
    let pool = get_postgres_client().await;
    let mut transaction = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin listing-source seed transaction: {error}"));

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("api-acceptance-party-{party_id}"))
        .bind(format!("API Acceptance Party {party_id}"))
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed party: {error}"));
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, url) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(listing_source_id)
    .bind(format!("api-acceptance-source-{listing_source_id}"))
    .bind(format!("API Acceptance Listing Source {listing_source_id}"))
    .bind(party_id)
    .bind("https://api-acceptance.example/")
    .execute(&mut *transaction)
    .await
    .unwrap_or_else(|error| panic!("failed to seed listing source: {error}"));
    sqlx::query(
        "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'PARTNER_API')",
    )
    .bind(listing_source_id)
    .execute(&mut *transaction)
    .await
    .unwrap_or_else(|error| panic!("failed to seed listing-source ingestion method: {error}"));
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(uuid::Uuid::new_v4())
        .bind(party_id)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed partnership: {error}"));
    transaction.commit().await.unwrap_or_else(|error| {
        panic!("failed to commit listing-source seed transaction: {error}")
    });
    listing_source_id
}

pub async fn seed_listing_source_for_search(
    name: &str,
    operator_name: &str,
    ingestion_method: &str,
    referral_configuration: Option<serde_json::Value>,
) -> (uuid::Uuid, uuid::Uuid, String) {
    let party_id = uuid::Uuid::new_v4();
    let listing_source_id = uuid::Uuid::new_v4();
    let listing_source_slug_id = format!("api-search-source-{listing_source_id}");
    let pool = get_postgres_client().await;
    let mut transaction = pool.begin().await.unwrap_or_else(|error| {
        panic!("failed to begin search listing-source seed transaction: {error}")
    });

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party_id)
        .bind(format!("api-search-party-{party_id}"))
        .bind(operator_name)
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed search operator party: {error}"));
    sqlx::query(
        "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, url, image, referral_configuration) VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(listing_source_id)
    .bind(&listing_source_slug_id)
    .bind(name)
    .bind(party_id)
    .bind("https://listing-source-search.example/")
    .bind("https://listing-source-search.example/image.jpg")
    .bind(referral_configuration)
    .execute(&mut *transaction)
    .await
    .unwrap_or_else(|error| panic!("failed to seed search listing source: {error}"));
    sqlx::query(
        "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, $2)",
    )
    .bind(listing_source_id)
    .bind(ingestion_method)
    .execute(&mut *transaction)
    .await
    .unwrap_or_else(|error| panic!("failed to seed search ingestion method: {error}"));
    if ingestion_method == "WOOCOMMERCE" {
        sqlx::query(
            "INSERT INTO listing_source_woocommerce_ingestion_configurations (listing_source_id, webhook_secret) VALUES ($1, $2)",
        )
        .bind(listing_source_id)
        .bind("provider-secret")
        .execute(&mut *transaction)
        .await
        .unwrap_or_else(|error| panic!("failed to seed search provider secret: {error}"));
    }
    transaction.commit().await.unwrap_or_else(|error| {
        panic!("failed to commit search listing-source seed transaction: {error}")
    });

    (listing_source_id, party_id, listing_source_slug_id)
}

pub async fn seed_product() -> ProductListingId {
    let listing_source_id = seed_listing_source().await;
    let product_listing_id = ProductListingId::new();
    let product_listing_title_slug_id = ProductListingSlugId::from_title_and_suffix(
        "acceptance product",
        &uuid::Uuid::from(product_listing_id).simple().to_string()[..6],
    )
    .unwrap_or_else(|error| panic!("valid fixture title slug: {error}"));
    let event_id = uuid::Uuid::new_v4();
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let mut tx = pool
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin product seed transaction: {error}"));
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO product_listings (
            product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id,
            embedding_source_event_id, listing_source_id, source_listing_id, availability, lifecycle, url
        ) VALUES ($1, $2, $3, $3, $3, $4, $5, 'AVAILABLE', 'ACTIVE', $6)
        "#,
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(product_listing_title_slug_id.as_ref())
    .bind(event_id)
    .bind(listing_source_id)
    .bind(format!("listing-source-product-{product_listing_id}"))
    .bind("https://api-acceptance.example/product")
    .execute(&mut *tx)
    .await
    {
        panic!("failed to seed product: {error}");
    }
    if let Err(error) = sqlx::query(
        r#"
        INSERT INTO product_listing_events (
            event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time
        )
        VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', $3, $4, now())
        "#,
    )
    .bind(event_id)
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(1_i16)
    .bind(serde_json::json!({
        "title": null,
        "description": null,
        "listingSourceId": listing_source_id.to_string(),
        "sourceListingId": format!("listing-source-product-{product_listing_id}"),
        "pricing": {
            "price": null,
            "priceEstimateMin": null,
            "priceEstimateMax": null
        },
        "availability": "AVAILABLE",
        "url": "https://api-acceptance.example/product",
        "imageCount": 0,
        "auction": {
            "start": null,
            "end": null
        }
    }))
    .execute(&mut *tx)
    .await
    {
        panic!("failed to seed product event: {error}");
    }
    if let Err(error) = tx.commit().await {
        panic!("failed to commit product seed transaction: {error}");
    }
    product_listing_id
}

pub(super) async fn seed_current_fx_snapshot(pool: &sqlx::PgPool) {
    let fx_rate_id = FxRateId::new();
    if let Err(error) = sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, now(), $2, $3)",
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind("fxratesapi")
    .bind(fx_rate_id.to_string())
    .execute(pool)
    .await
    {
        panic!("failed to seed current FX snapshot: {error}");
    }

    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF",
    ] {
        if let Err(error) = sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::from(fx_rate_id))
        .bind(currency)
        .bind(if currency == "EUR" {
            1_000_000_i64
        } else {
            1_250_000_i64
        })
        .execute(pool)
        .await
        {
            panic!("failed to seed current FX quote: {error}");
        }
    }
}

fn url(value: &str) -> Url {
    match Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("invalid test URL: {error}"),
    }
}

async fn test_state(search_embeddings: TestEmbeddingGenerator) -> AppState {
    let pool = get_postgres_client().await;
    seed_current_fx_snapshot(&pool).await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let access_token_use_case = user_service::use_cases::AuthenticateAccessTokenHandler::new(
        user_postgres::SqlxAccessTokenAuthenticationReader::new(pool.clone()),
    );
    let authenticator = Arc::new(ApiAuthService::new(
        RejectJwtAuthenticator,
        AuraAccessTokenAuthenticator::new(access_token_use_case),
    ));
    let opensearch_client = get_opensearch_client().await;
    let create_listing_source = CreateListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
    );
    let get_listing_source = GetListingSourceHandler::new(
        SqlxListingSourceReaders::new(pool.clone()),
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
    );
    let update_listing_source = UpdateListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceRepositoryFactory::new(),
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
    );
    let list_administered_listing_sources = ListAdministeredListingSourcesHandler::new(
        SqlxListingSourceAuthorization::new(pool.clone()),
    );
    let search_listing_sources = SearchListingSourcesHandler::new(
        unit_of_work.clone(),
        SqlxListingSourceSearchReaderFactory::new(),
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
    );
    let create_party = CreatePartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
    );
    let get_party = GetPartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
    );
    let update_party = UpdatePartyHandler::new(
        unit_of_work.clone(),
        SqlxPartyRepositoryFactory::new(),
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
    );
    let search_parties = SearchPartiesHandler::new(
        unit_of_work.clone(),
        SqlxPartySearchReaderFactory::new(),
        CheckUserAdminHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        ),
    );
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
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let list_admin_partnerships = ListAdminPartnershipsHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipSearchReaderFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let get_admin_partnership = GetAdminPartnershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipDetailsReaderFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let grant_partnership_membership = GrantPartnershipMembershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        user_postgres::SqlxUserAccountReaderFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let revoke_partnership_membership = RevokePartnershipMembershipHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        user_postgres::SqlxUserAccountReaderFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let grant_partnership_listing_source = GrantPartnershipListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let revoke_partnership_listing_source = RevokePartnershipListingSourceHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let get_partnership_application = GetPartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let mark_partnership_application_in_review = MarkPartnershipApplicationInReviewHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
    );
    let approve_partnership_application = ApprovePartnershipApplicationHandler::new(
        unit_of_work.clone(),
        SqlxPartnershipApplicationRepositoryFactory::new(),
        SqlxPartyRepositoryFactory::new(),
        SqlxListingSourceRepositoryFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxPartnershipRepositoryFactory::new(),
        SqlxListingSourceGrantRepositoryFactory::new(),
        user_postgres::SqlxUserAdminReaderFactory::new(),
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
        user_postgres::SqlxUserAdminReaderFactory::new(),
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
    );

    let products_state = ProductListingsState::new(
        Arc::new(GetProductListingHandler::new(
            unit_of_work.clone(),
            SqlxProductListingDetailsReaderFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        )),
        Arc::new(GetSimilarProductListingsHandler::new(
            unit_of_work.clone(),
            SqlxProductListingEmbeddingReaderFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
            OpenSearchProductListingSimilarProductListingsReader::new(opensearch_client.clone()),
            SqlxListingSourceSummaryReader::new(pool.clone()),
            SqlxProductListingUserStateReader::new(pool.clone()),
            SqlxProductListingContentAssessmentReader::new(pool.clone()),
        )),
        Arc::new(SearchProductListingsHandler::new(
            unit_of_work.clone(),
            OpenSearchProductListingSearchReader::new(opensearch_client.clone()),
            SqlxFxRateSnapshotRepositoryFactory,
            search_embeddings,
            SqlxListingSourceSummaryReader::new(pool.clone()),
            SqlxProductListingUserStateReader::new(pool.clone()),
            SqlxProductListingContentAssessmentReader::new(pool.clone()),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    )
    .with_product_listing_history(Arc::new(GetProductListingHistoryHandler::new(
        unit_of_work.clone(),
        SqlxProductListingHistoryReaderFactory::new(),
    )));

    let partner_product_listings_state = PartnerProductListingsState::new(
        Arc::new(CreateProductListingHandler::new(
            unit_of_work.clone(),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
        )),
        Arc::new(UpdateProductListingHandler::new(
            unit_of_work.clone(),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
        )),
        Arc::new(UpsertProductListingHandler::new(
            unit_of_work.clone(),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
        )),
        Arc::new(WithdrawProductListingHandler::new(
            unit_of_work.clone(),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let webhooks_state = WebhooksState::new(
        Arc::new(IngestWoocommerceProductListingHandler::new(
            unit_of_work.clone(),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
            SqlxListingSourceReaders::new(pool.clone()),
            SqlxListingSourceReaders::new(pool.clone()),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let listing_sources_state = ListingSourcesState::new(
        Arc::new(create_listing_source),
        Arc::new(get_listing_source),
        Arc::new(update_listing_source),
        Arc::new(list_administered_listing_sources),
        Arc::new(search_listing_sources),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    let parties_state = PartiesState::new(
        Arc::new(create_party),
        Arc::new(get_party),
        Arc::new(search_parties),
        Arc::new(update_party),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let users_state = UsersState::new(
        Arc::new(GetOwnUserHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAccountReaderFactory::new(),
        )),
        Arc::new(AdminGetUserHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserAccountReaderFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(SearchUsersHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserSearchReaderFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(UpdateUserProfileHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(UpdateUserProfileHandler::new_admin_only(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(ChangeUserRoleHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(ChangeUserTierHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(DeleteUserHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(DeleteUserHandler::new_admin_only(
            unit_of_work.clone(),
            user_postgres::SqlxUserRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(CreateAccessTokenHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxAccessTokenRepositoryFactory::new(),
        )),
        Arc::new(ListAccessTokensHandler::new(
            user_postgres::SqlxAccessTokenListReader::new(pool.clone()),
        )),
        Arc::new(GetAccessTokenHandler::new(
            user_postgres::SqlxAccessTokenDetailsReader::new(pool.clone()),
        )),
        Arc::new(UpdateAccessTokenHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxAccessTokenRepositoryFactory::new(),
        )),
        Arc::new(DeleteAccessTokenHandler::new(
            unit_of_work.clone(),
            user_postgres::SqlxAccessTokenRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::new(DeleteAccessTokenHandler::new_admin_only(
            unit_of_work.clone(),
            user_postgres::SqlxAccessTokenRepositoryFactory::new(),
            user_postgres::SqlxUserAdminReaderFactory::new(),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let search_filter_reader = SqlxSearchFilterReader::new(get_postgres_client().await);
    let search_filters_state = SearchFiltersState::new(
        Arc::new(ListOwnedSearchFiltersHandler::new(
            search_filter_reader.clone(),
        )),
        Arc::new(CreateSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            TestEmbeddingGenerator::Success,
            SqlxSearchFilterQuotaReaderFactory,
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(GetOwnedSearchFilterHandler::new(
            search_filter_reader.clone(),
        )),
        Arc::new(UpdateOwnedSearchFilterHandler::new(
            unit_of_work.clone(),
            SqlxSearchFilterRepositoryFactory,
            TestEmbeddingGenerator::Success,
            search_filter_reader.clone(),
            SqlxSearchFilterQuotaReaderFactory,
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
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

    let watchlist_state = WatchlistState::new(
        Arc::new(ListWatchlistHandler::new(
            unit_of_work.clone(),
            SqlxProductListingWatchlistDetailsReaderFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
        )),
        Arc::new(WatchProductListingHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistRepositoryFactory,
            SqlxWatchlistQuotaReaderFactory,
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(UpdateWatchlistProductListingHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistRepositoryFactory,
            SqlxWatchlistQuotaReaderFactory,
            user_postgres::SqlxUserTierEntitlementsFactory::new(),
        )),
        Arc::new(UnwatchProductListingHandler::new(
            unit_of_work.clone(),
            SqlxWatchlistRepositoryFactory,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let partnership_applications_state = PartnershipApplicationsState::new(
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
    );

    let billing_prices = BillingPriceIds {
        pro_monthly: "price_pro_monthly".to_owned(),
        pro_yearly: "price_pro_yearly".to_owned(),
        ultimate_monthly: "price_ultimate_monthly".to_owned(),
        ultimate_yearly: "price_ultimate_yearly".to_owned(),
    };
    let billing_state = BillingState::new(
        Arc::new(CreateBillingCheckoutSessionHandler::new(
            GetOwnUserHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAccountReaderFactory::new(),
            ),
            AssociateUserStripeCustomerIdHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserRepositoryFactory::new(),
            ),
            TestStripeBilling,
            TestStripeBilling,
            billing_prices.clone(),
        )),
        Arc::new(CreateBillingPortalSessionHandler::new(
            GetOwnUserHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAccountReaderFactory::new(),
            ),
            TestStripeBilling,
        )),
        Arc::new(CreateBillingManagementSessionHandler::new(
            GetOwnUserHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAccountReaderFactory::new(),
            ),
            AssociateUserStripeCustomerIdHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserRepositoryFactory::new(),
            ),
            TestStripeBilling,
            TestStripeBilling,
            TestStripeBilling,
            billing_prices,
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );

    let oauth_state = OAuthState::new(
        Arc::new(CreateOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAdminReaderFactory::new(),
            ),
        )),
        Arc::new(ListOAuthClientsHandler::new(
            SqlxOAuthClientListReader::new(pool.clone()),
            CheckUserAdminHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAdminReaderFactory::new(),
            ),
        )),
        Arc::new(GetOAuthClientHandler::new(
            SqlxOAuthClientDetailsReader::new(pool.clone()),
            CheckUserAdminHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAdminReaderFactory::new(),
            ),
        )),
        Arc::new(UpdateOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAdminReaderFactory::new(),
            ),
        )),
        Arc::new(DeleteOAuthClientHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            CheckUserAdminHandler::new(
                unit_of_work.clone(),
                user_postgres::SqlxUserAdminReaderFactory::new(),
            ),
        )),
        Arc::new(AuthorizeHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            SqlxAuthorizationCodeRepositoryFactory::new(),
        )),
        Arc::new(TokenByAuthorizationCodeHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            SqlxAuthorizationCodeRepositoryFactory::new(),
            SqlxThirdPartyExchangeCodeRepositoryFactory::new(),
            user_postgres::SqlxAccessTokenRepositoryFactory::new(),
        )),
        Arc::new(TokenByThirdPartyCodeHandler::new(
            unit_of_work.clone(),
            SqlxThirdPartyExchangeCodeRepositoryFactory::new(),
        )),
        Arc::new(RevokeTokenHandler::new(
            unit_of_work.clone(),
            SqlxOAuthClientRepositoryFactory::new(),
            user_postgres::SqlxAccessTokenRepositoryFactory::new(),
        )),
        Arc::new(IntrospectTokenHandler::new(
            SqlxOAuthClientAuthenticationReader::new(pool.clone()),
            user_postgres::SqlxAccessTokenAuthenticationReader::new(pool.clone()),
        )),
        Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
    );
    state::AppState::new()
        .with_admin_overview(AdminOverviewState::new(
            Arc::new(GetAdminOverviewHandler::new(
                unit_of_work.clone(),
                SqlxAdminOverviewReaderFactory::new(),
                user_postgres::SqlxUserAdminReaderFactory::new(),
            )),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_parties(parties_state)
        .with_users(users_state)
        .with_watchlist(watchlist_state)
        .with_partnership_applications(partnership_applications_state)
        .with_partnerships(partnerships_state)
        .with_listing_sources(listing_sources_state)
        .with_newsletter(NewsletterState::new(
            Arc::new(UpsertNewsletterSubscriptionHandler::new(
                user_postgres::SqlxNewsletterProfileReader::new(get_postgres_client().await),
                SuccessfulNewsletterWriter,
            )),
            Arc::clone(&authenticator) as Arc<dyn TokenAuthenticator>,
        ))
        .with_products(products_state)
        .with_partner_product_listings(partner_product_listings_state)
        .with_webhooks(webhooks_state)
        .with_oauth(oauth_state)
        .with_search_filters(search_filters_state)
        .with_notifications(notifications_state)
        .with_billing(billing_state)
}

struct RejectJwtAuthenticator;

#[async_trait::async_trait]
impl TokenAuthenticator for RejectJwtAuthenticator {
    async fn authenticate(
        &self,
        _bearer_token: &str,
        _metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        Err(AuthError::InvalidCredentials)
    }
}

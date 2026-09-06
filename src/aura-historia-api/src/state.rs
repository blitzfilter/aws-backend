use crate::auth::TokenAuthenticator;
use admin_overview_service::GetAdminOverviewUseCase;
use async_trait::async_trait;
use billing_service::use_cases::{
    CreateBillingCheckoutSessionUseCase, CreateBillingManagementSessionUseCase,
    CreateBillingPortalSessionUseCase,
};
use listing_source_service::use_cases::commands::create_listing_source::CreateListingSourceUseCase;
use listing_source_service::use_cases::commands::update_listing_source::UpdateListingSourceUseCase;
use listing_source_service::use_cases::queries::get_listing_source::GetListingSourceUseCase;
use listing_source_service::use_cases::queries::search_listing_sources::SearchListingSourcesUseCase;
use notification_service::use_cases::commands::delete_notification::DeleteNotificationUseCase;
use notification_service::use_cases::commands::delete_notifications::DeleteNotificationsUseCase;
use notification_service::use_cases::commands::update_all_notifications_seen::UpdateAllNotificationsSeenUseCase;
use notification_service::use_cases::commands::update_notification_seen::UpdateNotificationSeenUseCase;
use notification_service::use_cases::commands::update_notifications_seen::UpdateNotificationsSeenUseCase;
use notification_service::use_cases::queries::list_notifications::ListNotificationsUseCase;
use oauth_service::use_cases::{
    AuthorizeUseCase, CreateOAuthClientUseCase, DeleteOAuthClientUseCase, GetOAuthClientUseCase,
    IntrospectTokenUseCase, ListOAuthClientsUseCase, RevokeTokenUseCase,
    TokenByAuthorizationCodeUseCase, TokenByThirdPartyCodeUseCase, UpdateOAuthClientUseCase,
};
use partnership_service::use_cases::queries::get_admin_partnership::GetAdminPartnershipUseCase;
use partnership_service::use_cases::queries::list_admin_partnerships::ListAdminPartnershipsUseCase;
use partnership_service::use_cases::queries::list_administered_listing_sources::ListAdministeredListingSourcesUseCase;
use partnership_service::use_cases::{
    commands::{
        approve_partnership_application::ApprovePartnershipApplicationUseCase,
        grant_partnership_listing_source::GrantPartnershipListingSourceUseCase,
        grant_partnership_membership::GrantPartnershipMembershipUseCase,
        mark_partnership_application_in_review::MarkPartnershipApplicationInReviewUseCase,
        reject_partnership_application::RejectPartnershipApplicationUseCase,
        revoke_partnership_listing_source::RevokePartnershipListingSourceUseCase,
        revoke_partnership_membership::RevokePartnershipMembershipUseCase,
        submit_partnership_application::SubmitPartnershipApplicationUseCase,
        withdraw_partnership_application::WithdrawPartnershipApplicationUseCase,
    },
    queries::{
        get_own_partnership_application::GetOwnPartnershipApplicationUseCase,
        get_partnership_application::GetPartnershipApplicationUseCase,
        list_admin_partnership_applications::ListAdminPartnershipApplicationsUseCase,
        list_own_partnership_applications::ListOwnPartnershipApplicationsUseCase,
    },
};
use party_service::use_cases::commands::create_party::CreatePartyUseCase;
use party_service::use_cases::commands::update_party::UpdatePartyUseCase;
use party_service::use_cases::queries::get_party::GetPartyUseCase;
use party_service::use_cases::queries::search_parties::SearchPartiesUseCase;
use product_listing_service::use_cases::{
    CreateProductListingUseCase, GetProductListingHistoryUseCase, GetProductListingUseCase,
    GetSimilarProductListingsUseCase, IngestWoocommerceProductListingUseCase,
    SearchProductListingsUseCase, UpdateProductListingUseCase, UpsertProductListingUseCase,
    WithdrawProductListingUseCase,
};
use search_filter_service::use_cases::{
    CreateSearchFilterUseCase, DeleteOwnedSearchFilterUseCase, GetOwnedSearchFilterUseCase,
    ListOwnedSearchFiltersUseCase, ListSearchFilterMatchesUseCase, UpdateOwnedSearchFilterUseCase,
    UpdateSearchFilterMatchFeedbackUseCase,
};

use std::sync::Arc;
use user_service::use_cases::commands::change_user_role::ChangeUserRoleUseCase;
use user_service::use_cases::commands::change_user_tier::ChangeUserTierUseCase;
use user_service::use_cases::commands::create_access_token::CreateAccessTokenUseCase;
use user_service::use_cases::commands::delete_access_token::DeleteAccessTokenUseCase;
use user_service::use_cases::commands::delete_user::DeleteUserUseCase;
use user_service::use_cases::commands::update_access_token::UpdateAccessTokenUseCase;
use user_service::use_cases::commands::update_user_profile::UpdateUserProfileUseCase;
use user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionUseCase;
use user_service::use_cases::queries::admin_get_user::AdminGetUserUseCase;
use user_service::use_cases::queries::get_access_token::GetAccessTokenUseCase;
use user_service::use_cases::queries::get_own_user::GetOwnUserUseCase;
use user_service::use_cases::queries::list_access_tokens::ListAccessTokensUseCase;
use user_service::use_cases::queries::search_users::SearchUsersUseCase;
use watchlist_service::use_cases::{
    ListWatchlistUseCase, UnwatchProductListingUseCase, UpdateWatchlistProductListingUseCase,
    WatchProductListingUseCase,
};

#[async_trait]
pub(crate) trait ReadinessCheck: Send + Sync {
    async fn check(&self) -> Result<(), ()>;
}

#[derive(Clone, Copy)]
struct AlwaysReady;

#[async_trait]
impl ReadinessCheck for AlwaysReady {
    async fn check(&self) -> Result<(), ()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) readiness: Arc<dyn ReadinessCheck>,
    pub(crate) product_listings: Option<ProductListingsState>,
    pub(crate) partner_product_listings: Option<PartnerProductListingsState>,
    pub(crate) listing_sources: Option<ListingSourcesState>,
    pub(crate) admin_overview: Option<AdminOverviewState>,
    pub(crate) parties: Option<PartiesState>,
    pub(crate) users: Option<UsersState>,
    pub(crate) watchlist: Option<WatchlistState>,
    pub(crate) partnership_applications: Option<PartnershipApplicationsState>,
    pub(crate) partnerships: Option<PartnershipsState>,
    pub(crate) oauth: Option<OAuthState>,
    pub(crate) search_filters: Option<SearchFiltersState>,
    pub(crate) billing: Option<BillingState>,
    pub(crate) newsletter: Option<NewsletterState>,
    pub(crate) notifications: Option<NotificationsState>,
    pub(crate) webhooks: Option<WebhooksState>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            readiness: Arc::new(AlwaysReady),
            product_listings: None,
            partner_product_listings: None,
            listing_sources: None,
            admin_overview: None,
            parties: None,
            users: None,
            watchlist: None,
            partnership_applications: None,
            partnerships: None,
            oauth: None,
            search_filters: None,
            billing: None,
            newsletter: None,
            notifications: None,
            webhooks: None,
        }
    }

    pub(crate) fn with_readiness(mut self, readiness: Arc<dyn ReadinessCheck>) -> Self {
        self.readiness = readiness;
        self
    }

    pub fn with_products(mut self, product_listings: ProductListingsState) -> Self {
        self.product_listings = Some(product_listings);
        self
    }

    pub fn with_partner_product_listings(
        mut self,
        partner_product_listings: PartnerProductListingsState,
    ) -> Self {
        self.partner_product_listings = Some(partner_product_listings);
        self
    }

    pub fn with_listing_sources(mut self, listing_sources: ListingSourcesState) -> Self {
        self.listing_sources = Some(listing_sources);
        self
    }

    pub fn with_admin_overview(mut self, admin_overview: AdminOverviewState) -> Self {
        self.admin_overview = Some(admin_overview);
        self
    }

    pub fn with_users(mut self, users: UsersState) -> Self {
        self.users = Some(users);
        self
    }

    pub fn with_parties(mut self, parties: PartiesState) -> Self {
        self.parties = Some(parties);
        self
    }

    pub fn with_watchlist(mut self, watchlist: WatchlistState) -> Self {
        self.watchlist = Some(watchlist);
        self
    }

    pub fn with_partnership_applications(
        mut self,
        partnership_applications: PartnershipApplicationsState,
    ) -> Self {
        self.partnership_applications = Some(partnership_applications);
        self
    }

    pub fn with_partnerships(mut self, partnerships: PartnershipsState) -> Self {
        self.partnerships = Some(partnerships);
        self
    }

    pub fn with_oauth(mut self, oauth: OAuthState) -> Self {
        self.oauth = Some(oauth);
        self
    }

    pub fn with_search_filters(mut self, search_filters: SearchFiltersState) -> Self {
        self.search_filters = Some(search_filters);
        self
    }

    pub fn with_billing(mut self, billing: BillingState) -> Self {
        self.billing = Some(billing);
        self
    }

    pub fn with_newsletter(mut self, newsletter: NewsletterState) -> Self {
        self.newsletter = Some(newsletter);
        self
    }

    pub fn with_webhooks(mut self, webhooks: WebhooksState) -> Self {
        self.webhooks = Some(webhooks);
        self
    }

    pub fn with_notifications(mut self, notifications: NotificationsState) -> Self {
        self.notifications = Some(notifications);
        self
    }
}

#[derive(Clone)]
pub struct AdminOverviewState {
    pub(crate) get_overview: Arc<dyn GetAdminOverviewUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl AdminOverviewState {
    pub fn new(
        get_overview: Arc<dyn GetAdminOverviewUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            get_overview,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct NotificationsState {
    pub(crate) list_notifications: Arc<dyn ListNotificationsUseCase>,
    pub(crate) update_notification_seen: Arc<dyn UpdateNotificationSeenUseCase>,
    pub(crate) update_notifications_seen: Arc<dyn UpdateNotificationsSeenUseCase>,
    pub(crate) update_all_notifications_seen: Arc<dyn UpdateAllNotificationsSeenUseCase>,
    pub(crate) delete_notification: Arc<dyn DeleteNotificationUseCase>,
    pub(crate) delete_notifications: Arc<dyn DeleteNotificationsUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl NotificationsState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        list_notifications: Arc<dyn ListNotificationsUseCase>,
        update_notification_seen: Arc<dyn UpdateNotificationSeenUseCase>,
        update_notifications_seen: Arc<dyn UpdateNotificationsSeenUseCase>,
        update_all_notifications_seen: Arc<dyn UpdateAllNotificationsSeenUseCase>,
        delete_notification: Arc<dyn DeleteNotificationUseCase>,
        delete_notifications: Arc<dyn DeleteNotificationsUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            list_notifications,
            update_notification_seen,
            update_notifications_seen,
            update_all_notifications_seen,
            delete_notification,
            delete_notifications,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct WebhooksState {
    pub(crate) ingest: Arc<dyn IngestWoocommerceProductListingUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl WebhooksState {
    pub fn new(
        ingest: Arc<dyn IngestWoocommerceProductListingUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            ingest,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct BillingState {
    pub(crate) checkout: Arc<dyn CreateBillingCheckoutSessionUseCase>,
    pub(crate) portal: Arc<dyn CreateBillingPortalSessionUseCase>,
    pub(crate) manage: Arc<dyn CreateBillingManagementSessionUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl BillingState {
    pub fn new(
        checkout: Arc<dyn CreateBillingCheckoutSessionUseCase>,
        portal: Arc<dyn CreateBillingPortalSessionUseCase>,
        manage: Arc<dyn CreateBillingManagementSessionUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            checkout,
            portal,
            manage,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct NewsletterState {
    pub(crate) upsert_subscription: Arc<dyn UpsertNewsletterSubscriptionUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl NewsletterState {
    pub fn new(
        upsert_subscription: Arc<dyn UpsertNewsletterSubscriptionUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            upsert_subscription,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct SearchFiltersState {
    pub(crate) list_owned_search_filters: Arc<dyn ListOwnedSearchFiltersUseCase>,
    pub(crate) create_search_filter: Arc<dyn CreateSearchFilterUseCase>,
    pub(crate) get_owned_search_filter: Arc<dyn GetOwnedSearchFilterUseCase>,
    pub(crate) update_owned_search_filter: Arc<dyn UpdateOwnedSearchFilterUseCase>,
    pub(crate) delete_owned_search_filter: Arc<dyn DeleteOwnedSearchFilterUseCase>,
    pub(crate) list_search_filter_matches: Arc<dyn ListSearchFilterMatchesUseCase>,
    pub(crate) update_search_filter_match_feedback: Arc<dyn UpdateSearchFilterMatchFeedbackUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl SearchFiltersState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        list_owned_search_filters: Arc<dyn ListOwnedSearchFiltersUseCase>,
        create_search_filter: Arc<dyn CreateSearchFilterUseCase>,
        get_owned_search_filter: Arc<dyn GetOwnedSearchFilterUseCase>,
        update_owned_search_filter: Arc<dyn UpdateOwnedSearchFilterUseCase>,
        delete_owned_search_filter: Arc<dyn DeleteOwnedSearchFilterUseCase>,
        list_search_filter_matches: Arc<dyn ListSearchFilterMatchesUseCase>,
        update_search_filter_match_feedback: Arc<dyn UpdateSearchFilterMatchFeedbackUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            list_owned_search_filters,
            create_search_filter,
            get_owned_search_filter,
            update_owned_search_filter,
            delete_owned_search_filter,
            list_search_filter_matches,
            update_search_filter_match_feedback,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct OAuthState {
    pub(crate) create_client: Arc<dyn CreateOAuthClientUseCase>,
    pub(crate) list_clients: Arc<dyn ListOAuthClientsUseCase>,
    pub(crate) get_client: Arc<dyn GetOAuthClientUseCase>,
    pub(crate) update_client: Arc<dyn UpdateOAuthClientUseCase>,
    pub(crate) delete_client: Arc<dyn DeleteOAuthClientUseCase>,
    pub(crate) authorize: Arc<dyn AuthorizeUseCase>,
    pub(crate) token_by_authorization_code: Arc<dyn TokenByAuthorizationCodeUseCase>,
    pub(crate) token_by_third_party_code: Arc<dyn TokenByThirdPartyCodeUseCase>,
    pub(crate) revoke: Arc<dyn RevokeTokenUseCase>,
    pub(crate) introspect: Arc<dyn IntrospectTokenUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl OAuthState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        create_client: Arc<dyn CreateOAuthClientUseCase>,
        list_clients: Arc<dyn ListOAuthClientsUseCase>,
        get_client: Arc<dyn GetOAuthClientUseCase>,
        update_client: Arc<dyn UpdateOAuthClientUseCase>,
        delete_client: Arc<dyn DeleteOAuthClientUseCase>,
        authorize: Arc<dyn AuthorizeUseCase>,
        token_by_authorization_code: Arc<dyn TokenByAuthorizationCodeUseCase>,
        token_by_third_party_code: Arc<dyn TokenByThirdPartyCodeUseCase>,
        revoke: Arc<dyn RevokeTokenUseCase>,
        introspect: Arc<dyn IntrospectTokenUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            create_client,
            list_clients,
            get_client,
            update_client,
            delete_client,
            authorize,
            token_by_authorization_code,
            token_by_third_party_code,
            revoke,
            introspect,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct ProductListingsState {
    pub(crate) get_product: Arc<dyn GetProductListingUseCase>,
    pub(crate) get_product_listing_history: Option<Arc<dyn GetProductListingHistoryUseCase>>,
    pub(crate) get_similar_products: Arc<dyn GetSimilarProductListingsUseCase>,
    pub(crate) search_products: Arc<dyn SearchProductListingsUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl ProductListingsState {
    pub fn new(
        get_product: Arc<dyn GetProductListingUseCase>,
        get_similar_products: Arc<dyn GetSimilarProductListingsUseCase>,
        search_products: Arc<dyn SearchProductListingsUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            get_product,
            get_product_listing_history: None,
            get_similar_products,
            search_products,
            authenticator,
        }
    }

    pub fn with_product_listing_history(
        mut self,
        get_product_listing_history: Arc<dyn GetProductListingHistoryUseCase>,
    ) -> Self {
        self.get_product_listing_history = Some(get_product_listing_history);
        self
    }
}

#[derive(Clone)]
pub struct ListingSourcesState {
    pub(crate) create: Arc<dyn CreateListingSourceUseCase>,
    pub(crate) get: Arc<dyn GetListingSourceUseCase>,
    pub(crate) update: Arc<dyn UpdateListingSourceUseCase>,
    pub(crate) list_administered: Arc<dyn ListAdministeredListingSourcesUseCase>,
    pub(crate) search: Arc<dyn SearchListingSourcesUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl ListingSourcesState {
    pub fn new(
        create: Arc<dyn CreateListingSourceUseCase>,
        get: Arc<dyn GetListingSourceUseCase>,
        update: Arc<dyn UpdateListingSourceUseCase>,
        list_administered: Arc<dyn ListAdministeredListingSourcesUseCase>,
        search: Arc<dyn SearchListingSourcesUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            create,
            get,
            update,
            list_administered,
            search,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct PartiesState {
    pub(crate) create_party: Arc<dyn CreatePartyUseCase>,
    pub(crate) get_party: Arc<dyn GetPartyUseCase>,
    pub(crate) search_parties: Arc<dyn SearchPartiesUseCase>,
    pub(crate) update_party: Arc<dyn UpdatePartyUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl PartiesState {
    pub fn new(
        create_party: Arc<dyn CreatePartyUseCase>,
        get_party: Arc<dyn GetPartyUseCase>,
        search_parties: Arc<dyn SearchPartiesUseCase>,
        update_party: Arc<dyn UpdatePartyUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            create_party,
            get_party,
            search_parties,
            update_party,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct PartnerProductListingsState {
    pub(crate) create: Arc<dyn CreateProductListingUseCase>,
    pub(crate) update: Arc<dyn UpdateProductListingUseCase>,
    pub(crate) upsert: Arc<dyn UpsertProductListingUseCase>,
    pub(crate) withdraw: Arc<dyn WithdrawProductListingUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl PartnerProductListingsState {
    pub fn new(
        create: Arc<dyn CreateProductListingUseCase>,
        update: Arc<dyn UpdateProductListingUseCase>,
        upsert: Arc<dyn UpsertProductListingUseCase>,
        withdraw: Arc<dyn WithdrawProductListingUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            create,
            update,
            upsert,
            withdraw,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct UsersState {
    pub(crate) get_own_user: Arc<dyn GetOwnUserUseCase>,
    pub(crate) admin_get_user: Arc<dyn AdminGetUserUseCase>,
    pub(crate) search_users: Arc<dyn SearchUsersUseCase>,
    pub(crate) update_user_profile: Arc<dyn UpdateUserProfileUseCase>,
    pub(crate) admin_update_user_profile: Arc<dyn UpdateUserProfileUseCase>,
    pub(crate) change_user_role: Arc<dyn ChangeUserRoleUseCase>,
    pub(crate) change_user_tier: Arc<dyn ChangeUserTierUseCase>,
    pub(crate) delete_user: Arc<dyn DeleteUserUseCase>,
    pub(crate) admin_delete_user: Arc<dyn DeleteUserUseCase>,
    pub(crate) create_access_token: Arc<dyn CreateAccessTokenUseCase>,
    pub(crate) list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
    pub(crate) get_access_token: Arc<dyn GetAccessTokenUseCase>,
    pub(crate) update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
    pub(crate) delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
    pub(crate) admin_delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl UsersState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        get_own_user: Arc<dyn GetOwnUserUseCase>,
        admin_get_user: Arc<dyn AdminGetUserUseCase>,
        search_users: Arc<dyn SearchUsersUseCase>,
        update_user_profile: Arc<dyn UpdateUserProfileUseCase>,
        admin_update_user_profile: Arc<dyn UpdateUserProfileUseCase>,
        change_user_role: Arc<dyn ChangeUserRoleUseCase>,
        change_user_tier: Arc<dyn ChangeUserTierUseCase>,
        delete_user: Arc<dyn DeleteUserUseCase>,
        admin_delete_user: Arc<dyn DeleteUserUseCase>,
        create_access_token: Arc<dyn CreateAccessTokenUseCase>,
        list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
        get_access_token: Arc<dyn GetAccessTokenUseCase>,
        update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
        delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
        admin_delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            get_own_user,
            admin_get_user,
            search_users,
            update_user_profile,
            admin_update_user_profile,
            change_user_role,
            change_user_tier,
            delete_user,
            admin_delete_user,
            create_access_token,
            list_access_tokens,
            get_access_token,
            update_access_token,
            delete_access_token,
            admin_delete_access_token,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct WatchlistState {
    pub(crate) list_watchlist: Arc<dyn ListWatchlistUseCase>,
    pub(crate) watch_product: Arc<dyn WatchProductListingUseCase>,
    pub(crate) update_watchlist_product: Arc<dyn UpdateWatchlistProductListingUseCase>,
    pub(crate) unwatch_product: Arc<dyn UnwatchProductListingUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl WatchlistState {
    pub fn new(
        list_watchlist: Arc<dyn ListWatchlistUseCase>,
        watch_product: Arc<dyn WatchProductListingUseCase>,
        update_watchlist_product: Arc<dyn UpdateWatchlistProductListingUseCase>,
        unwatch_product: Arc<dyn UnwatchProductListingUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            list_watchlist,
            watch_product,
            update_watchlist_product,
            unwatch_product,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct PartnershipApplicationsState {
    pub(crate) submit: Arc<dyn SubmitPartnershipApplicationUseCase>,
    pub(crate) list_own: Arc<dyn ListOwnPartnershipApplicationsUseCase>,
    pub(crate) get_own: Arc<dyn GetOwnPartnershipApplicationUseCase>,
    pub(crate) withdraw: Arc<dyn WithdrawPartnershipApplicationUseCase>,
    pub(crate) list_admin: Arc<dyn ListAdminPartnershipApplicationsUseCase>,
    pub(crate) get: Arc<dyn GetPartnershipApplicationUseCase>,
    pub(crate) mark_in_review: Arc<dyn MarkPartnershipApplicationInReviewUseCase>,
    pub(crate) approve: Arc<dyn ApprovePartnershipApplicationUseCase>,
    pub(crate) reject: Arc<dyn RejectPartnershipApplicationUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl PartnershipApplicationsState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        submit: Arc<dyn SubmitPartnershipApplicationUseCase>,
        list_own: Arc<dyn ListOwnPartnershipApplicationsUseCase>,
        get_own: Arc<dyn GetOwnPartnershipApplicationUseCase>,
        withdraw: Arc<dyn WithdrawPartnershipApplicationUseCase>,
        list_admin: Arc<dyn ListAdminPartnershipApplicationsUseCase>,
        get: Arc<dyn GetPartnershipApplicationUseCase>,
        mark_in_review: Arc<dyn MarkPartnershipApplicationInReviewUseCase>,
        approve: Arc<dyn ApprovePartnershipApplicationUseCase>,
        reject: Arc<dyn RejectPartnershipApplicationUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            submit,
            list_own,
            get_own,
            withdraw,
            list_admin,
            get,
            mark_in_review,
            approve,
            reject,
            authenticator,
        }
    }
}

#[derive(Clone)]
pub struct PartnershipsState {
    pub(crate) list_admin: Arc<dyn ListAdminPartnershipsUseCase>,
    pub(crate) get_admin: Arc<dyn GetAdminPartnershipUseCase>,
    pub(crate) grant_member: Arc<dyn GrantPartnershipMembershipUseCase>,
    pub(crate) revoke_member: Arc<dyn RevokePartnershipMembershipUseCase>,
    pub(crate) grant_listing_source: Arc<dyn GrantPartnershipListingSourceUseCase>,
    pub(crate) revoke_listing_source: Arc<dyn RevokePartnershipListingSourceUseCase>,
    pub(crate) authenticator: Arc<dyn TokenAuthenticator>,
}

impl PartnershipsState {
    pub fn new(
        list_admin: Arc<dyn ListAdminPartnershipsUseCase>,
        get_admin: Arc<dyn GetAdminPartnershipUseCase>,
        grant_member: Arc<dyn GrantPartnershipMembershipUseCase>,
        revoke_member: Arc<dyn RevokePartnershipMembershipUseCase>,
        grant_listing_source: Arc<dyn GrantPartnershipListingSourceUseCase>,
        revoke_listing_source: Arc<dyn RevokePartnershipListingSourceUseCase>,
        authenticator: Arc<dyn TokenAuthenticator>,
    ) -> Self {
        Self {
            list_admin,
            get_admin,
            grant_member,
            revoke_member,
            grant_listing_source,
            revoke_listing_source,
            authenticator,
        }
    }
}

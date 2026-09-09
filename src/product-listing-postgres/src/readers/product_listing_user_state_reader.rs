use crate::object_id::{PersistedObjectIdError, try_from_uuid};
use application::error::{box_error, static_error};

use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingUserStateLookup, ProductListingUserStateReadError, ProductListingUserStateReader,
};
use product_listing_service::user_state::{
    ContentVisibilityUserState, NotificationUserState, ProductListingUserState,
    SearchFilterUserState, WatchlistUserState,
};
use search_filter_core::{
    enhanced_match_reason::EnhancedMatchReason, user_search_filter_name::UserSearchFilterName,
};
use sqlx::PgPool;
use std::collections::HashMap;

use user_core::tier::UserTier as CanonicalUserTier;

#[derive(Debug, Clone)]
pub struct SqlxProductListingUserStateReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingUserStateRow {
    product_listing_id: Option<uuid::Uuid>,
    user_show_unassessed_or_sensitive_content: bool,
    user_tier: String,
    watchlist_notifications: Option<bool>,
    selected_match_user_search_filter_id: Option<uuid::Uuid>,
    selected_match_user_search_filter_name: Option<String>,
    selected_match_reason: Option<String>,
    selected_match_feedback: Option<bool>,
    selected_match_month_position: Option<i64>,
    unseen_notification_ids: Option<Vec<uuid::Uuid>>,
}

#[derive(Debug, thiserror::Error)]
enum ProductListingUserStateRowMappingError {
    #[error("product user state row was returned without a requested product")]
    MissingRequestedProductListing,
    #[error("product user state has an invalid ProductListing ID")]
    InvalidProductListingId(#[source] PersistedObjectIdError),
    #[error("product user state has an invalid notification ID")]
    InvalidNotificationId(#[source] PersistedObjectIdError),
    #[error("product user state has an invalid user search filter ID")]
    InvalidUserSearchFilterId(#[source] PersistedObjectIdError),
    #[error("product user state has an invalid tier")]
    InvalidTier,
    #[error("unmatched product user state contains match fields")]
    UnmatchedProductListingHasMatchFields,
    #[error("free-tier matched product user state is missing monthly match position")]
    MissingFreeTierMonthPosition,
    #[error("unlimited-tier matched product user state has a monthly match position")]
    UnexpectedUnlimitedTierMonthPosition,
}

#[derive(Debug, Clone, Copy)]
enum UserTier {
    Free,
    Unlimited,
}

impl SqlxProductListingUserStateReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductListingUserStateReader for SqlxProductListingUserStateReader {
    async fn find_for_user(
        &self,
        lookup: &ProductListingUserStateLookup,
    ) -> Result<HashMap<ProductListingId, ProductListingUserState>, ProductListingUserStateReadError>
    {
        let product_listing_ids = lookup
            .product_listing_ids
            .iter()
            .copied()
            .map(|id| id.into_uuid())
            .collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, ProductListingUserStateRow>(SELECT_PRODUCT_USER_STATES)
            .bind(lookup.user_id.as_uuid())
            .bind(product_listing_ids)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| ProductListingUserStateReadError::QueryFailed {
                source: box_error(source),
            })?;

        if rows.is_empty() {
            return Err(ProductListingUserStateReadError::InvalidReadModel {
                source: static_error("product user state user is missing"),
            });
        }

        if lookup.product_listing_ids.is_empty() {
            let [row] = rows.as_slice() else {
                return Err(ProductListingUserStateReadError::InvalidReadModel {
                    source: static_error("empty product user state lookup returned product rows"),
                });
            };
            user_tier(&row.user_tier).map_err(|source| {
                ProductListingUserStateReadError::InvalidReadModel {
                    source: box_error(source),
                }
            })?;

            if row.product_listing_id.is_some() {
                return Err(ProductListingUserStateReadError::InvalidReadModel {
                    source: static_error("empty product user state lookup returned a product"),
                });
            }

            return Ok(HashMap::new());
        }

        rows.into_iter()
            .map(product_user_state)
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(
                |source| ProductListingUserStateReadError::InvalidReadModel {
                    source: box_error(source),
                },
            )
    }
}

const SELECT_PRODUCT_USER_STATES: &str = r#"
    WITH requested_products AS (
        SELECT DISTINCT requested.product_listing_id
        FROM UNNEST($2::uuid[]) AS requested(product_listing_id)
    ),
    requested_rows AS (
        SELECT product_listing_id
        FROM requested_products

        UNION ALL

        SELECT NULL::uuid
        WHERE NOT EXISTS (SELECT 1 FROM requested_products)
    ),
    authenticated_user AS (
        SELECT show_unassessed_or_sensitive_content, tier
        FROM users
        WHERE user_id = $1
    ),
    notification_states AS (
        SELECT
            notification.product_listing_id,
            array_agg(
                notification.notification_id
                ORDER BY notification.created DESC, notification.notification_id DESC
            ) AS unseen_notification_ids
        FROM notifications notification
        JOIN requested_products requested
            ON requested.product_listing_id = notification.product_listing_id
        WHERE notification.user_id = $1
            AND notification.seen = false
        GROUP BY notification.product_listing_id
    ),
    ranked_requested_matches AS (
        SELECT
            matched.product_listing_id,
            matched.user_search_filter_id,
            matched.user_search_filter_name,
            matched.enhanced_match_reason,
            matched.feedback,
            matched.created,
            ROW_NUMBER() OVER (
                PARTITION BY matched.product_listing_id
                ORDER BY matched.created ASC, matched.user_search_filter_id ASC
            ) AS product_position
        FROM search_filter_matches matched
        JOIN requested_products requested
            ON requested.product_listing_id = matched.product_listing_id
        WHERE matched.user_id = $1
    ),
    selected_matches AS (
        SELECT
            product_listing_id,
            user_search_filter_id,
            user_search_filter_name,
            enhanced_match_reason,
            feedback,
            created,
            date_trunc('month', created AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS month_start
        FROM ranked_requested_matches
        WHERE product_position = 1
    ),
    selected_match_months AS (
        SELECT DISTINCT month_start
        FROM selected_matches
    ),
    ranked_month_matches AS (
        SELECT
            matched.product_listing_id,
            matched.user_search_filter_id,
            ROW_NUMBER() OVER (
                PARTITION BY date_trunc('month', matched.created AT TIME ZONE 'UTC')
                ORDER BY
                    matched.created ASC,
                    matched.user_search_filter_id ASC,
                    matched.product_listing_id ASC
            ) AS month_position
        FROM authenticated_user authenticated_user
        JOIN search_filter_matches matched
            ON matched.user_id = $1
        JOIN selected_match_months month
            ON matched.created >= month.month_start
            AND matched.created < month.month_start + INTERVAL '1 month'
        WHERE authenticated_user.tier = 'FREE'
    )
    SELECT
        requested.product_listing_id,
        authenticated_user.show_unassessed_or_sensitive_content AS user_show_unassessed_or_sensitive_content,
        authenticated_user.tier AS user_tier,
        watchlist.notifications AS watchlist_notifications,
        selected_match.user_search_filter_id AS selected_match_user_search_filter_id,
        selected_match.user_search_filter_name AS selected_match_user_search_filter_name,
        selected_match.enhanced_match_reason AS selected_match_reason,
        selected_match.feedback AS selected_match_feedback,
        CASE
            WHEN authenticated_user.tier = 'FREE' THEN monthly_match.month_position
            ELSE NULL
        END AS selected_match_month_position,
        notification_state.unseen_notification_ids
    FROM authenticated_user authenticated_user
    CROSS JOIN requested_rows requested

    LEFT JOIN product_listing_watchlist watchlist
        ON watchlist.user_id = $1
        AND watchlist.product_listing_id = requested.product_listing_id
    LEFT JOIN selected_matches selected_match
        ON selected_match.product_listing_id = requested.product_listing_id
    LEFT JOIN ranked_month_matches monthly_match
        ON monthly_match.product_listing_id = selected_match.product_listing_id
        AND monthly_match.user_search_filter_id = selected_match.user_search_filter_id
    LEFT JOIN notification_states notification_state
        ON notification_state.product_listing_id = requested.product_listing_id
"#;

fn product_user_state(
    row: ProductListingUserStateRow,
) -> Result<(ProductListingId, ProductListingUserState), ProductListingUserStateRowMappingError> {
    let product_listing_id = row
        .product_listing_id
        .ok_or(ProductListingUserStateRowMappingError::MissingRequestedProductListing)
        .and_then(|id| {
            try_from_uuid(id, "ProductListing ID")
                .map_err(ProductListingUserStateRowMappingError::InvalidProductListingId)
        })?;

    let tier = user_tier(&row.user_tier)?;
    let search_filter = search_filter_user_state(&row, tier)?;

    Ok((
        product_listing_id,
        ProductListingUserState {
            watchlist: WatchlistUserState {
                watching: row.watchlist_notifications.is_some(),
                notifications: row.watchlist_notifications.unwrap_or(false),
            },
            content_visibility: ContentVisibilityUserState {
                show_unassessed_or_sensitive_content: row.user_show_unassessed_or_sensitive_content,
            },
            notification: NotificationUserState {
                unseen_notification_ids: row
                    .unseen_notification_ids
                    .unwrap_or_default()
                    .into_iter()
                    .map(|id| {
                        try_from_uuid(id, "notification ID")
                            .map_err(ProductListingUserStateRowMappingError::InvalidNotificationId)
                    })
                    .collect::<Result<_, _>>()?,
            },
            search_filter,
        },
    ))
}

fn user_tier(value: &str) -> Result<UserTier, ProductListingUserStateRowMappingError> {
    CanonicalUserTier::from_code(value)
        .map(|tier| match tier {
            CanonicalUserTier::Free => UserTier::Free,
            CanonicalUserTier::Pro | CanonicalUserTier::Ultimate => UserTier::Unlimited,
        })
        .ok_or(ProductListingUserStateRowMappingError::InvalidTier)
}

fn search_filter_user_state(
    row: &ProductListingUserStateRow,
    tier: UserTier,
) -> Result<SearchFilterUserState, ProductListingUserStateRowMappingError> {
    let Some(user_search_filter_id) = row.selected_match_user_search_filter_id else {
        if row.selected_match_user_search_filter_name.is_some()
            || row.selected_match_reason.is_some()
            || row.selected_match_feedback.is_some()
            || row.selected_match_month_position.is_some()
        {
            return Err(
                ProductListingUserStateRowMappingError::UnmatchedProductListingHasMatchFields,
            );
        }

        return Ok(SearchFilterUserState::default());
    };

    let hidden = match tier {
        UserTier::Free => {
            row.selected_match_month_position
                .ok_or(ProductListingUserStateRowMappingError::MissingFreeTierMonthPosition)?
                > 10
        }
        UserTier::Unlimited => {
            if row.selected_match_month_position.is_some() {
                return Err(
                    ProductListingUserStateRowMappingError::UnexpectedUnlimitedTierMonthPosition,
                );
            }
            false
        }
    };

    Ok(SearchFilterUserState {
        matched: true,
        hidden,
        user_search_filter_id: Some(
            try_from_uuid(user_search_filter_id, "user search filter ID")
                .map_err(ProductListingUserStateRowMappingError::InvalidUserSearchFilterId)?,
        ),
        user_search_filter_name: row
            .selected_match_user_search_filter_name
            .clone()
            .map(UserSearchFilterName::from),
        match_reason: row
            .selected_match_reason
            .clone()
            .map(EnhancedMatchReason::from),
        match_feedback: row.selected_match_feedback,
    })
}

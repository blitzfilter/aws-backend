use crate::mapping::{UserRowMappingError, bind_tier, parse_tier};
use application::error::box_error;
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;
use user_core::tier::UserTier;
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

const RECONCILE_WATCHLIST_SQL: &str = r#"
WITH locked AS MATERIALIZED (
    SELECT
        product_listing_id,
        created,
        state
    FROM product_listing_watchlist
    WHERE user_id = $1
      AND state IN ('ACTIVE', 'INACTIVE_BY_RESTRICTED_PLAN')
    FOR UPDATE
), ranked AS (
    SELECT
        product_listing_id,
        CASE
            WHEN row_number() OVER (ORDER BY created DESC, product_listing_id ASC) <= $2
                THEN 'ACTIVE'
            ELSE 'INACTIVE_BY_RESTRICTED_PLAN'
        END AS target_state
    FROM locked
)
UPDATE product_listing_watchlist AS entry
SET state = ranked.target_state,
    active_since = CASE
        WHEN ranked.target_state = 'ACTIVE' THEN now()
        ELSE NULL
    END,
    version = entry.version + 1,
    updated = now()
FROM ranked
WHERE entry.user_id = $1
  AND entry.product_listing_id = ranked.product_listing_id
  AND entry.state IS DISTINCT FROM ranked.target_state
"#;

const RECONCILE_SEARCH_FILTERS_SQL: &str = r#"
WITH candidates AS (
    SELECT
        user_search_filter_id,
        created,
        CASE
            WHEN $3 = 'ULTIMATE' THEN true
            WHEN $3 = 'PRO' THEN enhanced_search_description IS NULL
            WHEN $3 = 'FREE' THEN
                enhanced_search_description IS NULL
                AND coalesce(jsonb_array_length(search -> 'listing_source_id_query'), 0) = 0
                AND coalesce(jsonb_array_length(search -> 'exclude_listing_source_id_query'), 0) = 0
                AND search ->> 'created_query' IS NULL
                AND search ->> 'updated_query' IS NULL
                AND search ->> 'lot_bidding_opens_query' IS NULL
                AND search ->> 'lot_scheduled_closes_query' IS NULL
            ELSE false
        END AS feature_allowed
    FROM search_filters
    WHERE user_id = $1
      AND state IN ('ACTIVE', 'INACTIVE_BY_RESTRICTED_PLAN')
), ranked AS (
    SELECT
        user_search_filter_id,
        feature_allowed,
        sum(CASE WHEN feature_allowed THEN 1 ELSE 0 END)
            OVER (ORDER BY created DESC, user_search_filter_id ASC) AS eligible_rank
    FROM candidates
), reconciled AS (
    SELECT
        user_search_filter_id,
        CASE
            WHEN feature_allowed AND eligible_rank <= $2 THEN 'ACTIVE'
            ELSE 'INACTIVE_BY_RESTRICTED_PLAN'
        END AS target_state
    FROM ranked
)
UPDATE search_filters AS filter
SET state = reconciled.target_state,
    version = filter.version + 1,
    updated = now()
FROM reconciled
WHERE filter.user_id = $1
  AND filter.user_search_filter_id = reconciled.user_search_filter_id
  AND filter.state IS DISTINCT FROM reconciled.target_state
"#;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserTierEntitlementsFactory;

struct SqlxUserTierEntitlements<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
enum SqlxUserTierEntitlementsError {
    #[error("failed to lock authoritative user tier")]
    LockUserTier(#[source] sqlx::Error),
    #[error("invalid persisted user tier")]
    InvalidUserTier(#[source] UserRowMappingError),
    #[error("failed to reconcile watchlist tier entitlements")]
    ReconcileWatchlist(#[source] sqlx::Error),
    #[error("failed to reconcile search filter tier entitlements")]
    ReconcileSearchFilters(#[source] sqlx::Error),
}

impl SqlxUserTierEntitlementsFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserTierEntitlementsFactory<SqlxTransaction> for SqlxUserTierEntitlementsFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl UserTierEntitlements + 'tx {
        SqlxUserTierEntitlements {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserTierEntitlements for SqlxUserTierEntitlements<'_> {
    async fn lock_user_tier(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<UserTier>, UserTierEntitlementsError> {
        let tier =
            sqlx::query_scalar::<_, String>("SELECT tier FROM users WHERE user_id = $1 FOR UPDATE")
                .bind(user_id.into_uuid())
                .fetch_optional(&mut *self.connection)
                .await
                .map_err(|source| UserTierEntitlementsError::LockFailed {
                    source: box_error(SqlxUserTierEntitlementsError::LockUserTier(source)),
                })?;

        tier.map(|tier| parse_tier(&tier))
            .transpose()
            .map_err(|source| UserTierEntitlementsError::LockFailed {
                source: box_error(SqlxUserTierEntitlementsError::InvalidUserTier(source)),
            })
    }

    async fn reconcile_for_tier(
        &mut self,
        user_id: UserId,
        tier: UserTier,
    ) -> Result<(), UserTierEntitlementsError> {
        let quota = watchlist_quota(tier);
        sqlx::query(RECONCILE_WATCHLIST_SQL)
            .bind(user_id.into_uuid())
            .bind(quota)
            .execute(&mut *self.connection)
            .await
            .map_err(|source| UserTierEntitlementsError::ReconciliationFailed {
                source: box_error(SqlxUserTierEntitlementsError::ReconcileWatchlist(source)),
            })?;

        let quota = search_filter_quota(tier);
        sqlx::query(RECONCILE_SEARCH_FILTERS_SQL)
            .bind(user_id.into_uuid())
            .bind(quota)
            .bind(bind_tier(tier))
            .execute(&mut *self.connection)
            .await
            .map_err(|source| UserTierEntitlementsError::ReconciliationFailed {
                source: box_error(SqlxUserTierEntitlementsError::ReconcileSearchFilters(
                    source,
                )),
            })?;

        Ok(())
    }
}

fn watchlist_quota(tier: UserTier) -> i64 {
    match tier {
        UserTier::Free => 20,
        UserTier::Pro => 100,
        UserTier::Ultimate => i64::MAX,
    }
}

fn search_filter_quota(tier: UserTier) -> i64 {
    match tier {
        UserTier::Free => 1,
        UserTier::Pro => 5,
        UserTier::Ultimate => i64::MAX,
    }
}

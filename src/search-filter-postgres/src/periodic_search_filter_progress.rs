use application::error::box_error;
use platform_postgres::SqlxTransaction;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    PeriodicSearchFilterProgress, PeriodicSearchFilterProgressError,
    PeriodicSearchFilterProgressFactory, PeriodicSearchFilterProgressLockOutcome,
    PeriodicSearchFilterProgressWriteOutcome,
};
use time::OffsetDateTime;

#[derive(Debug, Clone, Default)]
pub struct SqlxPeriodicSearchFilterProgressFactory;
struct SqlxPeriodicSearchFilterProgress<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl PeriodicSearchFilterProgressFactory<SqlxTransaction>
    for SqlxPeriodicSearchFilterProgressFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PeriodicSearchFilterProgress + 'tx {
        SqlxPeriodicSearchFilterProgress { tx }
    }
}

#[async_trait::async_trait]
impl PeriodicSearchFilterProgress for SqlxPeriodicSearchFilterProgress<'_> {
    async fn lock_and_read(
        &mut self,
        search_filter_id: UserSearchFilterId,
        expected_version: i64,
        created: OffsetDateTime,
        window_end: OffsetDateTime,
    ) -> Result<PeriodicSearchFilterProgressLockOutcome, PeriodicSearchFilterProgressError> {
        let id = search_filter_id.into_uuid();
        let version = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM search_filters WHERE user_search_filter_id = $1 AND state = 'ACTIVE' FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(self.tx.connection())
        .await
        .map_err(|source| PeriodicSearchFilterProgressError::PersistenceFailed {
            source: box_error(source),
        })?;
        if version != Some(expected_version) {
            return Ok(PeriodicSearchFilterProgressLockOutcome::ChangedOrInactive);
        }
        let matched_through = sqlx::query_scalar::<_, OffsetDateTime>("SELECT matched_through FROM search_filter_periodic_match_state WHERE user_search_filter_id = $1 FOR UPDATE")
            .bind(id).fetch_optional(self.tx.connection()).await
            .map_err(|source| PeriodicSearchFilterProgressError::PersistenceFailed { source: box_error(source) })?
            .unwrap_or(created);
        let _ = window_end;
        Ok(PeriodicSearchFilterProgressLockOutcome::Current { matched_through })
    }

    async fn compare_and_set(
        &mut self,
        search_filter_id: UserSearchFilterId,
        expected: OffsetDateTime,
        matched_through: OffsetDateTime,
    ) -> Result<PeriodicSearchFilterProgressWriteOutcome, PeriodicSearchFilterProgressError> {
        if matched_through <= expected {
            return Ok(PeriodicSearchFilterProgressWriteOutcome::AlreadyCovered);
        }
        let id = search_filter_id.into_uuid();
        let changed = sqlx::query_scalar::<_, uuid::Uuid>(
            r#"
            INSERT INTO search_filter_periodic_match_state (user_search_filter_id, matched_through)
            SELECT $1, $3
            WHERE $2 < $3
            ON CONFLICT (user_search_filter_id) DO UPDATE
              SET matched_through = EXCLUDED.matched_through, updated = now()
              WHERE search_filter_periodic_match_state.matched_through = $2
                AND search_filter_periodic_match_state.matched_through < EXCLUDED.matched_through
            RETURNING user_search_filter_id
        "#,
        )
        .bind(id)
        .bind(expected)
        .bind(matched_through)
        .fetch_optional(self.tx.connection())
        .await
        .map_err(
            |source| PeriodicSearchFilterProgressError::PersistenceFailed {
                source: box_error(source),
            },
        )?;
        Ok(if changed.is_some() {
            PeriodicSearchFilterProgressWriteOutcome::Advanced
        } else {
            PeriodicSearchFilterProgressWriteOutcome::Superseded
        })
    }
}

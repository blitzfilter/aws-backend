use application::error::box_error;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use search_filter_service::ports::{
    SearchFilterMonthlyMatchQuotaReadError, SearchFilterMonthlyMatchQuotaReader,
    SearchFilterMonthlyMatchQuotaReaderFactory,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterMonthlyMatchQuotaReaderFactory;

struct SqlxSearchFilterMonthlyMatchQuotaReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl SearchFilterMonthlyMatchQuotaReaderFactory<SqlxTransaction>
    for SqlxSearchFilterMonthlyMatchQuotaReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterMonthlyMatchQuotaReader + 'tx {
        SqlxSearchFilterMonthlyMatchQuotaReader { tx }
    }
}

#[async_trait::async_trait]
impl SearchFilterMonthlyMatchQuotaReader for SqlxSearchFilterMonthlyMatchQuotaReader<'_> {
    async fn notification_selection_rank_for_user_in_month(
        &mut self,
        user_id: UserId,
        matched_at: OffsetDateTime,
        origin_event_id: EventId,
    ) -> Result<usize, SearchFilterMonthlyMatchQuotaReadError> {
        let rank = sqlx::query_scalar::<_, i64>(
            r#"
            WITH selected_events AS (
                SELECT DISTINCT ON (origin_event_id)
                    origin_event_id,
                    created
                FROM search_filter_matches
                WHERE user_id = $1
                    AND created >= date_trunc('month', $2::timestamptz)
                    AND created < date_trunc('month', $2::timestamptz) + INTERVAL '1 month'
                ORDER BY origin_event_id, user_search_filter_id ASC
            )
            SELECT count(*)
            FROM selected_events
            WHERE (created, origin_event_id) <= ($2, $3)
            "#,
        )
        .bind(user_id.into_uuid())
        .bind(matched_at)
        .bind(origin_event_id.into_uuid())
        .fetch_one(self.tx.connection())
        .await
        .map_err(
            |source| SearchFilterMonthlyMatchQuotaReadError::ReadFailed {
                source: box_error(source),
            },
        )?;

        usize::try_from(rank).map_err(
            |source| SearchFilterMonthlyMatchQuotaReadError::ReadFailed {
                source: box_error(source),
            },
        )
    }
}

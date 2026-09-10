use crate::mapping::{name, product_search_from_json, state};
use application::error::box_error;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    PeriodicSearchFilterCandidate, PeriodicSearchFilterCandidatePageRequest,
    PeriodicSearchFilterCandidateReadError, PeriodicSearchFilterCandidateReader,
};
use sqlx::{FromRow, PgPool};
use time::OffsetDateTime;

#[derive(Clone)]
pub struct SqlxPeriodicSearchFilterCandidateReader {
    pool: PgPool,
}

impl SqlxPeriodicSearchFilterCandidateReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, FromRow)]
struct PeriodicCandidateRow {
    user_search_filter_id: uuid::Uuid,
    user_id: uuid::Uuid,
    name: String,
    state: String,
    search: serde_json::Value,
    embedding: Option<Vec<f32>>,
    created: OffsetDateTime,
    matched_through: OffsetDateTime,
    version: i64,
}

#[async_trait::async_trait]
impl PeriodicSearchFilterCandidateReader for SqlxPeriodicSearchFilterCandidateReader {
    async fn find_active_page(
        &self,
        request: PeriodicSearchFilterCandidatePageRequest,
    ) -> Result<Vec<PeriodicSearchFilterCandidate>, PeriodicSearchFilterCandidateReadError> {
        let after = request.after.map(UserSearchFilterId::into_uuid);
        let limit = i64::try_from(request.page_size).map_err(|source| {
            PeriodicSearchFilterCandidateReadError::ReadFailed {
                source: box_error(source),
            }
        })?;
        sqlx::query_as::<_, PeriodicCandidateRow>(
            r#"
            SELECT filter.user_search_filter_id, filter.user_id, filter.name, filter.state,
                   filter.search, filter.embedding, filter.created,
                   COALESCE(progress.matched_through, filter.created) AS matched_through,
                   filter.version
            FROM search_filters filter
            LEFT JOIN search_filter_periodic_match_state progress
              ON progress.user_search_filter_id = filter.user_search_filter_id
            WHERE filter.state = 'ACTIVE'
              AND filter.enhanced_search_description IS NOT NULL
              AND filter.created <= $1
              AND (progress.matched_through IS NULL OR progress.matched_through < $1)
              AND ($2::uuid IS NULL OR filter.user_search_filter_id > $2)
            ORDER BY filter.user_search_filter_id ASC
            LIMIT $3
        "#,
        )
        .bind(request.eligible_at_or_before)
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(
            |source| PeriodicSearchFilterCandidateReadError::ReadFailed {
                source: box_error(source),
            },
        )?
        .into_iter()
        .map(|row| {
            Ok(PeriodicSearchFilterCandidate {
                search_filter_id: UserSearchFilterId::try_from(row.user_search_filter_id).map_err(
                    |source| PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    },
                )?,
                user_id: user_core::user_id::UserId::try_from(row.user_id).map_err(|source| {
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                name: name(row.name).map_err(|source| {
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                version: row.version,
                state: state(&row.state).map_err(|source| {
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                search: product_search_from_json(row.search).map_err(|source| {
                    PeriodicSearchFilterCandidateReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                embedding: row.embedding,
                created: row.created,
                matched_through: row.matched_through,
            })
        })
        .collect()
    }
}

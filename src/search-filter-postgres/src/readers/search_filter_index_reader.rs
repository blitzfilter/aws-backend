use crate::mapping::{FILTER_COLUMNS, FilterRow};
use application::error::box_error;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    SearchFilterIndexReadError, SearchFilterIndexReader, SearchFilterProjection,
};
use sqlx::{PgPool, Postgres, QueryBuilder};

#[derive(Clone)]
pub struct SqlxSearchFilterIndexReader {
    pool: PgPool,
}

impl SqlxSearchFilterIndexReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl SearchFilterIndexReader for SqlxSearchFilterIndexReader {
    async fn find_by_id(
        &self,
        search_filter_id: UserSearchFilterId,
    ) -> Result<Option<SearchFilterProjection>, SearchFilterIndexReadError> {
        let search_filter_id = search_filter_id.into_uuid();
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(FILTER_COLUMNS)
            .push(" FROM search_filters WHERE user_search_filter_id=$1");
        query
            .build_query_as::<FilterRow>()
            .bind(search_filter_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|source| SearchFilterIndexReadError::ReadFailed {
                source: box_error(source),
            })?
            .map(FilterRow::into_projection)
            .transpose()
    }

    async fn list_after(
        &self,
        after: Option<UserSearchFilterId>,
        limit: usize,
    ) -> Result<Vec<SearchFilterProjection>, SearchFilterIndexReadError> {
        let after = after.map(UserSearchFilterId::into_uuid);
        let limit =
            i64::try_from(limit).map_err(|source| SearchFilterIndexReadError::ReadFailed {
                source: box_error(source),
            })?;
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query.push(FILTER_COLUMNS).push(
            " FROM search_filters \
             WHERE ($1::uuid IS NULL OR user_search_filter_id > $1) \
             ORDER BY user_search_filter_id ASC LIMIT $2",
        );
        query
            .build_query_as::<FilterRow>()
            .bind(after)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|source| SearchFilterIndexReadError::ReadFailed {
                source: box_error(source),
            })?
            .into_iter()
            .map(FilterRow::into_projection)
            .collect()
    }
}

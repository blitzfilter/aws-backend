use crate::mapping::{FILTER_COLUMNS, FilterRow};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{SearchFilterReadError, SearchFilterReader, SearchFilterView};
use sqlx::{PgPool, Postgres, QueryBuilder};
use user_core::user_id::UserId;

#[derive(Clone)]
pub struct SqlxSearchFilterReader {
    pub(super) pool: PgPool,
}

impl SqlxSearchFilterReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl SearchFilterReader for SqlxSearchFilterReader {
    async fn find_for_user(
        &self,
        user_id: UserId,
    ) -> Result<Vec<SearchFilterView>, SearchFilterReadError> {
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(FILTER_COLUMNS)
            .push(" FROM search_filters WHERE user_id=$1 ORDER BY created DESC");
        query
            .build_query_as::<FilterRow>()
            .bind(user_id.into_uuid())
            .fetch_all(&self.pool)
            .await
            .map_err(|_| SearchFilterReadError::ReadFailed)?
            .into_iter()
            .map(FilterRow::into_view)
            .collect()
    }

    async fn find_for_user_by_id(
        &self,
        user_id: UserId,
        id: UserSearchFilterId,
    ) -> Result<Option<SearchFilterView>, SearchFilterReadError> {
        let id = id.into_uuid();
        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(FILTER_COLUMNS)
            .push(" FROM search_filters WHERE user_id=$1 AND user_search_filter_id=$2");
        query
            .build_query_as::<FilterRow>()
            .bind(user_id.into_uuid())
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| SearchFilterReadError::ReadFailed)?
            .map(FilterRow::into_view)
            .transpose()
    }
}

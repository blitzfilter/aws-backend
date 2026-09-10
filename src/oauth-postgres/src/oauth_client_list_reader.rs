use crate::mapping::OAUTH_CLIENT_VIEW_COLUMNS;
use crate::rows::OAuthClientViewRow;
use application::error::box_error;
use application::pagination::Cursor;
use oauth_core::OAuthClientSearch;
use oauth_service::ports::{OAuthClientListReader, OAuthClientReadError, OAuthClientView};
use oauth_service::use_cases::{
    ListOAuthClientsRequest, ListOAuthClientsResult, OAuthClientSearchCursor,
};
use sqlx::{PgPool, Postgres, QueryBuilder};

const MAX_CURSOR_SIZE: u64 = 100;

#[derive(Clone)]
pub struct SqlxOAuthClientListReader {
    pool: PgPool,
}

impl SqlxOAuthClientListReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl OAuthClientListReader for SqlxOAuthClientListReader {
    async fn search(
        &self,
        request: &ListOAuthClientsRequest,
    ) -> Result<ListOAuthClientsResult, OAuthClientReadError> {
        let cursor = request.cursor.unwrap_or_default();
        let size = cursor.size.clamp(1, MAX_CURSOR_SIZE);
        // `size` is clamped to 1..=100, so these casts are safe on supported targets.
        let size_usize = size as usize;
        let limit = (size + 1) as i64;

        let mut query = QueryBuilder::<Postgres>::new("SELECT ");
        query
            .push(OAUTH_CLIENT_VIEW_COLUMNS)
            .push(" FROM oauth_clients WHERE TRUE");
        push_filters(&mut query, &request.search);
        if let Some(search_after) = cursor.search_after {
            query
                .push(" AND (created, client_id) > (")
                .push_bind(search_after.position)
                .push(", ")
                .push_bind(search_after.client_id.into_uuid())
                .push(")");
        }
        query
            .push(" ORDER BY created ASC, client_id ASC LIMIT ")
            .push_bind(limit);

        let mut rows = query
            .build_query_as::<OAuthClientViewRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(temporarily_unavailable)?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items: Vec<OAuthClientView> = rows
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map_err(invalid_persisted_state)?;
        let search_after = if has_more {
            items.last().map(|item| OAuthClientSearchCursor {
                position: item.created,
                client_id: item.client_id,
            })
        } else {
            None
        };

        Ok(ListOAuthClientsResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(query: &mut QueryBuilder<Postgres>, search: &OAuthClientSearch) {
    if let Some(client_id) = search.client_id {
        query
            .push(" AND client_id = ")
            .push_bind(client_id.into_uuid());
    }
    if let Some(name) = &search.name_query {
        query
            .push(" AND name ILIKE ")
            .push_bind(like_pattern(name.as_ref()))
            .push(r" ESCAPE E'\\'");
    }
}

fn like_pattern(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    pattern.push('%');
    for character in value.chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

fn temporarily_unavailable(source: sqlx::Error) -> OAuthClientReadError {
    OAuthClientReadError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn invalid_persisted_state(
    source: impl std::error::Error + Send + Sync + 'static,
) -> OAuthClientReadError {
    OAuthClientReadError::InvalidPersistedState {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_escape_like_wildcards_as_literal_text() {
        assert_eq!(r"%100\%\_ready\\\\%", like_pattern(r"100%_ready\\"));
    }
}

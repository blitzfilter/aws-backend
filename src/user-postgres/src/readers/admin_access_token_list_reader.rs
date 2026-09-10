use crate::access_token_mapping::AccessTokenDetailsRow;
use application::error::box_error;
use application::pagination::{Cursor, CursoredResult};
use platform_postgres::SqlxTransaction;
use sqlx::{PgConnection, Postgres, QueryBuilder};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenDetails, AdminAccessTokenListReadError, AdminAccessTokenListReader,
    AdminAccessTokenListReaderFactory,
};
use user_service::use_cases::queries::list_admin_access_tokens::AccessTokenSearchCursor;

const MAX_CURSOR_SIZE: u64 = 100;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAdminAccessTokenListReaderFactory;

struct SqlxAdminAccessTokenListReader<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxAdminAccessTokenListReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AdminAccessTokenListReaderFactory<SqlxTransaction> for SqlxAdminAccessTokenListReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AdminAccessTokenListReader + 'tx {
        SqlxAdminAccessTokenListReader {
            connection: tx.connection(),
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct AdminAccessTokenDetailsRow {
    access_token_id: uuid::Uuid,
    user_id: uuid::Uuid,
    name: String,
    scopes: Vec<String>,
    origin: String,
    oauth_client_id: Option<uuid::Uuid>,
    expires_at: Option<OffsetDateTime>,
    created: OffsetDateTime,
}

#[async_trait::async_trait]
impl AdminAccessTokenListReader for SqlxAdminAccessTokenListReader<'_> {
    async fn list_for_user(
        &mut self,
        user_id: UserId,
        cursor: Cursor<AccessTokenSearchCursor>,
    ) -> Result<
        CursoredResult<AccessTokenDetails, AccessTokenSearchCursor>,
        AdminAccessTokenListReadError,
    > {
        let size = cursor.size.clamp(1, MAX_CURSOR_SIZE);
        let size_usize =
            usize::try_from(size).map_err(|source| AdminAccessTokenListReadError::Internal {
                source: box_error(source),
            })?;
        let limit =
            i64::try_from(size + 1).map_err(|source| AdminAccessTokenListReadError::Internal {
                source: box_error(source),
            })?;

        let mut query = QueryBuilder::<Postgres>::new(
            "SELECT access_token_id, user_id, name, scopes, origin, oauth_client_id, expires_at, created FROM access_tokens WHERE user_id = ",
        );
        query.push_bind(user_id.into_uuid());
        if let Some(search_after) = cursor.search_after {
            query
                .push(" AND (created, access_token_id) > (")
                .push_bind(search_after.position)
                .push(", ")
                .push_bind(search_after.access_token_id.into_uuid())
                .push(")");
        }
        query
            .push(" ORDER BY created ASC, access_token_id ASC LIMIT ")
            .push_bind(limit);

        let mut rows = query
            .build_query_as::<AdminAccessTokenDetailsRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(
                |source| AdminAccessTokenListReadError::TemporarilyUnavailable {
                    source: box_error(source),
                },
            )?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items_with_position = rows
            .into_iter()
            .map(|row| {
                let created = row.created;
                AccessTokenDetails::try_from(AccessTokenDetailsRow {
                    access_token_id: row.access_token_id,
                    user_id: row.user_id,
                    name: row.name,
                    scopes: row.scopes,
                    origin: row.origin,
                    oauth_client_id: row.oauth_client_id,
                    expires_at: row.expires_at,
                })
                .map(|details| (details, created))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| AdminAccessTokenListReadError::InvalidReadModel {
                source: box_error(source),
            })?;

        let search_after = if has_more {
            items_with_position
                .last()
                .map(|(details, created)| AccessTokenSearchCursor {
                    position: *created,
                    access_token_id: details.access_token_id,
                })
        } else {
            None
        };
        let items = items_with_position
            .into_iter()
            .map(|(details, _)| details)
            .collect();

        Ok(CursoredResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

use crate::mapping::{UserRow, bind_role, bind_tier, sort_user_field_columns, user_columns};
use application::error::box_error;
use application::pagination::Cursor;
use domain_primitives::sort::{Sort, SortOrder};
use platform_postgres::SqlxTransaction;
use sqlx::{Postgres, QueryBuilder};
use user_core::sort_user_field::SortUserField;

use user_service::ports::{UserSearchReadError, UserSearchReader, UserSearchReaderFactory};
use user_service::use_cases::queries::search_users::{
    SearchUsersRequest, SearchUsersResult, UserSummary,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxUserSearchReaderFactory;

struct SqlxUserSearchReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxUserSearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl UserSearchReaderFactory<SqlxTransaction> for SqlxUserSearchReaderFactory {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut SqlxTransaction) -> impl UserSearchReader + 'tx {
        SqlxUserSearchReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl UserSearchReader for SqlxUserSearchReader<'_> {
    async fn search(
        &mut self,
        request: &SearchUsersRequest,
    ) -> Result<SearchUsersResult, UserSearchReadError> {
        let cursor = request.cursor.unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize = usize::try_from(size).map_err(|source| UserSearchReadError::Internal {
            source: box_error(source),
        })?;
        let limit = i64::try_from(size + 1).map_err(|source| UserSearchReadError::Internal {
            source: box_error(source),
        })?;
        let sort = request.sort.unwrap_or(Sort {
            sort: SortUserField::default(),
            order: SortOrder::Asc,
        });

        let mut builder = QueryBuilder::<Postgres>::new("WITH filtered AS (SELECT ");
        builder.push(user_columns()).push(" FROM users WHERE TRUE");
        push_filters(&mut builder, request);
        builder.push("), ranked AS (SELECT filtered.*, row_number() OVER (ORDER BY ");
        push_sort_fields(&mut builder, sort);
        builder.push(", user_id ASC) AS rn FROM filtered) SELECT ");
        builder.push(user_columns()).push(" FROM ranked WHERE TRUE");
        if let Some(search_after) = cursor.search_after {
            builder.push(" AND rn > (SELECT rn FROM ranked WHERE user_id = ");
            builder.push_bind(search_after.into_uuid());
            builder.push(")");
        }
        builder.push(" ORDER BY rn LIMIT ").push_bind(limit);

        let mut rows = builder
            .build_query_as::<UserRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(|source| UserSearchReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items = rows
            .into_iter()
            .map(UserSummary::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| UserSearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after = if has_more {
            items.last().map(|item| item.user_id)
        } else {
            None
        };

        Ok(SearchUsersResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, request: &SearchUsersRequest) {
    let search = &request.search;

    if let Some(query) = &search.query {
        builder
            .push(" AND (email ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()))
            .push(" OR first_name ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()))
            .push(" OR last_name ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()))
            .push(")");
    }
    if let Some(query) = &search.email_query {
        builder
            .push(" AND email ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()));
    }
    if let Some(query) = &search.first_name_query {
        builder
            .push(" AND first_name ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()));
    }
    if let Some(query) = &search.last_name_query {
        builder
            .push(" AND last_name ILIKE ")
            .push_bind(format!("%{}%", query.as_ref()));
    }
    if !search.tier_query.is_empty() {
        let tiers = search
            .tier_query
            .iter()
            .copied()
            .map(bind_tier)
            .collect::<Vec<_>>();
        builder.push(" AND tier = ANY(").push_bind(tiers).push(")");
    }
    if !search.role_query.is_empty() {
        let roles = search
            .role_query
            .iter()
            .copied()
            .map(bind_role)
            .collect::<Vec<_>>();
        builder.push(" AND role = ANY(").push_bind(roles).push(")");
    }

    if let Some(created) = search.created {
        if let Some(min) = created.min {
            builder.push(" AND created >= ").push_bind(min);
        }
        if let Some(max) = created.max {
            builder.push(" AND created <= ").push_bind(max);
        }
    }
    if let Some(updated) = search.updated {
        if let Some(min) = updated.min {
            builder.push(" AND updated >= ").push_bind(min);
        }
        if let Some(max) = updated.max {
            builder.push(" AND updated <= ").push_bind(max);
        }
    }
}

fn push_sort_fields(builder: &mut QueryBuilder<Postgres>, sort: Sort<SortUserField>) {
    let order = match sort.order {
        SortOrder::Asc => "ASC",
        SortOrder::Desc => "DESC",
    };

    for (index, column) in sort_user_field_columns(sort.sort).iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push(*column).push(" ").push(order);
    }
}

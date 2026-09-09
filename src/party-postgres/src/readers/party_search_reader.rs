use crate::mapping::{PartyRow, party_columns, sort_party_field_columns};
use application::error::box_error;
use application::pagination::Cursor;

use party_core::sort_party_field::SortPartyField;
use party_service::ports::{PartySearchReadError, PartySearchReader, PartySearchReaderFactory};
use party_service::use_cases::queries::search_parties::{
    PartySummary, SearchPartiesRequest, SearchPartiesResult,
};
use platform_postgres::SqlxTransaction;
use sqlx::{Postgres, QueryBuilder};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPartySearchReaderFactory;

struct SqlxPartySearchReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl SqlxPartySearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PartySearchReaderFactory<SqlxTransaction> for SqlxPartySearchReaderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PartySearchReader + 'tx {
        SqlxPartySearchReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PartySearchReader for SqlxPartySearchReader<'_> {
    async fn search(
        &mut self,
        request: &SearchPartiesRequest,
    ) -> Result<SearchPartiesResult, PartySearchReadError> {
        let cursor = request.cursor.unwrap_or_default();
        let size = cursor.size.clamp(1, 100);
        let size_usize =
            usize::try_from(size).map_err(|source| PartySearchReadError::Internal {
                source: box_error(source),
            })?;
        let limit = i64::try_from(size + 1).map_err(|source| PartySearchReadError::Internal {
            source: box_error(source),
        })?;
        let sort_field = request
            .sort
            .map_or(SortPartyField::default(), |sort| sort.sort);
        let order = request.sort.map_or("asc", |sort| sort.order.as_str());

        let mut builder = QueryBuilder::<Postgres>::new("WITH filtered AS (SELECT ");
        builder
            .push(party_columns())
            .push(" FROM parties WHERE TRUE");
        push_filters(&mut builder, request);
        builder.push("), ranked AS (SELECT filtered.*, row_number() OVER (ORDER BY ");
        push_sort_fields(&mut builder, sort_field, order);
        builder.push(", party_id ASC) AS rn FROM filtered) SELECT ");
        builder
            .push(party_columns())
            .push(" FROM ranked WHERE TRUE");
        if let Some(search_after) = cursor.search_after {
            builder.push(" AND rn > (SELECT rn FROM ranked WHERE party_id = ");
            builder.push_bind(search_after.into_uuid());
            builder.push(")");
        }
        builder.push(" ORDER BY rn LIMIT ").push_bind(limit);

        let mut rows = builder
            .build_query_as::<PartyRow>()
            .fetch_all(&mut *self.connection)
            .await
            .map_err(|source| PartySearchReadError::TemporarilyUnavailable {
                source: box_error(source),
            })?;

        let has_more = rows.len() > size_usize;
        if has_more {
            rows.truncate(size_usize);
        }
        let items = rows
            .into_iter()
            .map(PartySummary::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| PartySearchReadError::InvalidReadModel {
                source: box_error(source),
            })?;
        let search_after = if has_more {
            items.last().map(|item| item.party_id)
        } else {
            None
        };

        Ok(SearchPartiesResult {
            items,
            cursor: Cursor { size, search_after },
            total: None,
        })
    }
}

fn push_filters(builder: &mut QueryBuilder<Postgres>, request: &SearchPartiesRequest) {
    let search = &request.search;

    if let Some(query) = &search.query {
        builder.push(" AND (");
        push_ilike(builder, "name", query.as_ref());
        builder.push(" OR ");
        push_ilike(builder, "phone", query.as_ref());
        builder.push(" OR ");
        push_ilike(builder, "email", query.as_ref());
        builder.push(")");
    }
    if let Some(query) = &search.name_query {
        builder.push(" AND ");
        push_ilike(builder, "name", query.as_ref());
    }
    if let Some(query) = &search.phone_query {
        builder.push(" AND ");
        push_ilike(builder, "phone", query.as_ref());
    }
    if let Some(query) = &search.email_query {
        builder.push(" AND ");
        push_ilike(builder, "email", query.as_ref());
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

fn push_ilike(builder: &mut QueryBuilder<Postgres>, column: &str, value: &str) {
    builder
        .push(column)
        .push(" ILIKE ")
        .push_bind(like_pattern(value))
        .push(r" ESCAPE E'\\'");
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

fn push_sort_fields(builder: &mut QueryBuilder<Postgres>, sort_field: SortPartyField, order: &str) {
    for (index, column) in sort_party_field_columns(sort_field).iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder
            .push(*column)
            .push(" ")
            .push(order)
            .push(" NULLS LAST");
    }
}

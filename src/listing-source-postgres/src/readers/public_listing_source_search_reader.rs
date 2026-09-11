use super::public_listing_source::{PublicListingSourceRow, map_public_listing_source};
use application::error::box_error;
use listing_source_service::{
    ports::{
        PublicListingSourceSearchReadError, PublicListingSourceSearchReader,
        PublicListingSourceSearchReaderFactory,
    },
    use_cases::queries::{
        public_listing_source::PublicListingSourceSummary,
        search_public_listing_sources::{
            PublicListingSourceSearchPage, PublicListingSourceSearchPosition,
            SearchPublicListingSourcesRequest,
        },
    },
};
use platform_postgres::SqlxTransaction;
use sqlx::{Postgres, QueryBuilder};

const STATEMENT_TIMEOUT: &str = "150ms";
const BROWSE_SQL: &str = "SELECT s.listing_source_id, s.listing_source_slug_id, s.name, p.name AS operator_name, s.url, s.image, 0::smallint AS match_tier, s.name_search FROM listing_sources s JOIN parties p ON p.party_id = s.operator_party_id WHERE TRUE";
const PREFIX_SQL_AFTER_INPUT: &str = "::text AS prefix_pattern), candidates AS (SELECT s.listing_source_id, CASE WHEN s.name_search = i.query THEN 0::smallint ELSE 1::smallint END AS match_tier FROM listing_sources s CROSS JOIN input i WHERE s.name_search LIKE i.prefix_pattern ESCAPE E'\\\\' UNION ALL SELECT s.listing_source_id, CASE WHEN p.name_search = i.query THEN 3::smallint ELSE 4::smallint END AS match_tier FROM parties p JOIN listing_sources s ON s.operator_party_id = p.party_id CROSS JOIN input i WHERE p.name_search LIKE i.prefix_pattern ESCAPE E'\\\\'), best AS (SELECT listing_source_id, min(match_tier) AS match_tier FROM candidates GROUP BY listing_source_id) SELECT s.listing_source_id, s.listing_source_slug_id, s.name, p.name AS operator_name, s.url, s.image, b.match_tier, s.name_search FROM best b JOIN listing_sources s ON s.listing_source_id = b.listing_source_id JOIN parties p ON p.party_id = s.operator_party_id WHERE TRUE";
const CONTAINS_SQL_AFTER_INPUT: &str = "::text AS contains_pattern), candidates AS (SELECT s.listing_source_id, CASE WHEN s.name_search = i.query THEN 0::smallint WHEN s.name_search LIKE i.prefix_pattern ESCAPE E'\\\\' THEN 1::smallint ELSE 2::smallint END AS match_tier FROM listing_sources s CROSS JOIN input i WHERE s.name_search LIKE i.contains_pattern ESCAPE E'\\\\' UNION ALL SELECT s.listing_source_id, CASE WHEN p.name_search = i.query THEN 3::smallint WHEN p.name_search LIKE i.prefix_pattern ESCAPE E'\\\\' THEN 4::smallint ELSE 5::smallint END AS match_tier FROM parties p JOIN listing_sources s ON s.operator_party_id = p.party_id CROSS JOIN input i WHERE p.name_search LIKE i.contains_pattern ESCAPE E'\\\\'), best AS (SELECT listing_source_id, min(match_tier) AS match_tier FROM candidates GROUP BY listing_source_id) SELECT s.listing_source_id, s.listing_source_slug_id, s.name, p.name AS operator_name, s.url, s.image, b.match_tier, s.name_search FROM best b JOIN listing_sources s ON s.listing_source_id = b.listing_source_id JOIN parties p ON p.party_id = s.operator_party_id WHERE TRUE";

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxPublicListingSourceSearchReaderFactory;

struct SqlxPublicListingSourceSearchReader<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct PublicListingSourceSearchRow {
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    name: String,
    operator_name: String,
    url: Option<String>,
    image: Option<String>,
    match_tier: i16,
    name_search: String,
}

#[derive(Debug, thiserror::Error)]
enum PublicListingSourceSearchRowMappingError {
    #[error("invalid public ListingSource search match tier persisted")]
    MatchTier,
    #[error("invalid public ListingSource search position")]
    Position(#[source] listing_source_service::use_cases::queries::search_public_listing_sources::PublicListingSourceSearchPositionError),
    #[error("invalid public ListingSource summary")]
    Summary(#[source] super::public_listing_source::PublicListingSourceMappingError),
}

impl SqlxPublicListingSourceSearchReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl PublicListingSourceSearchReaderFactory<SqlxTransaction>
    for SqlxPublicListingSourceSearchReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl PublicListingSourceSearchReader + 'tx {
        SqlxPublicListingSourceSearchReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl PublicListingSourceSearchReader for SqlxPublicListingSourceSearchReader<'_> {
    async fn search(
        &mut self,
        request: &SearchPublicListingSourcesRequest,
    ) -> Result<PublicListingSourceSearchPage, PublicListingSourceSearchReadError> {
        set_statement_timeout(self.connection).await?;

        let rows = if request.query().is_browse() {
            browse(self.connection, request).await?
        } else {
            let normalized_query = normalize_query(
                self.connection,
                request.query().canonical_text().unwrap_or_default(),
            )
            .await?;
            if normalized_query.is_empty() {
                Vec::new()
            } else if has_three_consecutive_alphanumeric(&normalized_query) {
                contains(self.connection, request, &normalized_query).await?
            } else {
                prefix(self.connection, request, &normalized_query).await?
            }
        };

        page(rows, request.page_size())
    }
}

async fn set_statement_timeout(
    connection: &mut sqlx::PgConnection,
) -> Result<(), PublicListingSourceSearchReadError> {
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(STATEMENT_TIMEOUT)
        .execute(connection)
        .await
        .map_err(temporary)?;
    Ok(())
}

async fn normalize_query(
    connection: &mut sqlx::PgConnection,
    query: &str,
) -> Result<String, PublicListingSourceSearchReadError> {
    sqlx::query_scalar::<_, String>("SELECT public.aura_search_name($1)")
        .bind(query)
        .fetch_one(connection)
        .await
        .map_err(temporary)
}

async fn browse(
    connection: &mut sqlx::PgConnection,
    request: &SearchPublicListingSourcesRequest,
) -> Result<Vec<PublicListingSourceSearchRow>, PublicListingSourceSearchReadError> {
    let mut builder = QueryBuilder::<Postgres>::new(BROWSE_SQL);
    if let Some(continuation) = request.continuation() {
        let position = continuation.position();
        builder.push(" AND (s.name_search COLLATE \"C\" > ");
        builder.push_bind(position.name_search());
        builder.push(" COLLATE \"C\" OR (s.name_search COLLATE \"C\" = ");
        builder.push_bind(position.name_search());
        builder.push(" COLLATE \"C\" AND s.listing_source_id > ");
        builder.push_bind(position.listing_source_id().into_uuid());
        builder.push("))");
    }
    builder.push(" ORDER BY s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT ");
    builder.push_bind(i64::from(request.page_size()) + 1);

    builder
        .build_query_as::<PublicListingSourceSearchRow>()
        .fetch_all(connection)
        .await
        .map_err(temporary)
}

async fn prefix(
    connection: &mut sqlx::PgConnection,
    request: &SearchPublicListingSourcesRequest,
    query: &str,
) -> Result<Vec<PublicListingSourceSearchRow>, PublicListingSourceSearchReadError> {
    let prefix_pattern = format!("{}%", escaped_like(query));
    let mut builder = QueryBuilder::<Postgres>::new("WITH input AS (SELECT ");
    builder
        .push_bind(query)
        .push("::text AS query, ")
        .push_bind(prefix_pattern)
        .push(PREFIX_SQL_AFTER_INPUT);
    push_text_cursor(&mut builder, request);
    builder.push(" ORDER BY b.match_tier ASC, s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT ");
    builder.push_bind(i64::from(request.page_size()) + 1);

    builder
        .build_query_as::<PublicListingSourceSearchRow>()
        .fetch_all(connection)
        .await
        .map_err(temporary)
}

async fn contains(
    connection: &mut sqlx::PgConnection,
    request: &SearchPublicListingSourcesRequest,
    query: &str,
) -> Result<Vec<PublicListingSourceSearchRow>, PublicListingSourceSearchReadError> {
    let escaped = escaped_like(query);
    let prefix_pattern = format!("{escaped}%");
    let contains_pattern = format!("%{escaped}%");
    let mut builder = QueryBuilder::<Postgres>::new("WITH input AS (SELECT ");
    builder
        .push_bind(query)
        .push("::text AS query, ")
        .push_bind(prefix_pattern)
        .push("::text AS prefix_pattern, ")
        .push_bind(contains_pattern)
        .push(CONTAINS_SQL_AFTER_INPUT);
    push_text_cursor(&mut builder, request);
    builder.push(" ORDER BY b.match_tier ASC, s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT ");
    builder.push_bind(i64::from(request.page_size()) + 1);

    builder
        .build_query_as::<PublicListingSourceSearchRow>()
        .fetch_all(connection)
        .await
        .map_err(temporary)
}

fn push_text_cursor(
    builder: &mut QueryBuilder<Postgres>,
    request: &SearchPublicListingSourcesRequest,
) {
    let Some(continuation) = request.continuation() else {
        return;
    };
    let position = continuation.position();
    builder.push(" AND (b.match_tier > ");
    builder.push_bind(i16::from(position.match_tier()));
    builder.push(" OR (b.match_tier = ");
    builder.push_bind(i16::from(position.match_tier()));
    builder.push(" AND s.name_search COLLATE \"C\" > ");
    builder.push_bind(position.name_search());
    builder.push(" COLLATE \"C\") OR (b.match_tier = ");
    builder.push_bind(i16::from(position.match_tier()));
    builder.push(" AND s.name_search COLLATE \"C\" = ");
    builder.push_bind(position.name_search());
    builder.push(" COLLATE \"C\" AND s.listing_source_id > ");
    builder.push_bind(position.listing_source_id().into_uuid());
    builder.push("))");
}

fn page(
    rows: Vec<PublicListingSourceSearchRow>,
    page_size: u8,
) -> Result<PublicListingSourceSearchPage, PublicListingSourceSearchReadError> {
    let mut rows = rows
        .into_iter()
        .map(map_search_row)
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid)?;
    let has_more = rows.len() > usize::from(page_size);
    if has_more {
        rows.truncate(usize::from(page_size));
    }
    let next_position = has_more
        .then(|| rows.last().map(|(_, position)| position.clone()))
        .flatten();
    let items = rows.into_iter().map(|(summary, _)| summary).collect();

    Ok(PublicListingSourceSearchPage {
        items,
        next_position,
    })
}

fn map_search_row(
    row: PublicListingSourceSearchRow,
) -> Result<
    (
        PublicListingSourceSummary,
        PublicListingSourceSearchPosition,
    ),
    PublicListingSourceSearchRowMappingError,
> {
    let match_tier = u8::try_from(row.match_tier)
        .ok()
        .filter(|tier| *tier <= 5)
        .ok_or(PublicListingSourceSearchRowMappingError::MatchTier)?;
    let position = PublicListingSourceSearchPosition::new(
        match_tier,
        row.name_search,
        listing_source_core::ListingSourceId::try_from(row.listing_source_id)
            .map_err(|_| PublicListingSourceSearchRowMappingError::MatchTier)?,
    )
    .map_err(PublicListingSourceSearchRowMappingError::Position)?;
    let summary = map_public_listing_source(PublicListingSourceRow {
        listing_source_id: row.listing_source_id,
        listing_source_slug_id: row.listing_source_slug_id,
        name: row.name,
        operator_name: row.operator_name,
        url: row.url,
        image: row.image,
    })
    .map_err(PublicListingSourceSearchRowMappingError::Summary)?;

    Ok((summary, position))
}

fn escaped_like(query: &str) -> String {
    let mut pattern = String::with_capacity(query.len());
    for character in query.chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

fn has_three_consecutive_alphanumeric(query: &str) -> bool {
    let mut run = 0;
    for character in query.chars() {
        if character.is_alphanumeric() {
            run += 1;
            if run == 3 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

fn temporary(source: sqlx::Error) -> PublicListingSourceSearchReadError {
    PublicListingSourceSearchReadError::TemporarilyUnavailable {
        source: box_error(source),
    }
}

fn invalid(
    source: impl std::error::Error + Send + Sync + 'static,
) -> PublicListingSourceSearchReadError {
    PublicListingSourceSearchReadError::InvalidReadModel {
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use listing_source_service::use_cases::queries::search_public_listing_sources::{
        PublicListingSourceSearchQuery, SearchPublicListingSourcesHandler,
        SearchPublicListingSourcesRequest, SearchPublicListingSourcesResult,
        SearchPublicListingSourcesUseCase,
    };
    use platform_postgres::SqlxUnitOfWork;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    #[test]
    fn should_escape_literal_like_metacharacters() {
        assert_eq!(r"100\%\_ready\\\\", escaped_like(r"100%_ready\\"));
    }

    #[test]
    fn should_select_contains_only_for_a_three_character_alphanumeric_run() {
        for query in ["mul", "CJK中文", "абв"] {
            assert!(has_three_consecutive_alphanumeric(query));
        }
        for query in ["mu", "a-b", "ab cd", "--"] {
            assert!(!has_three_consecutive_alphanumeric(query));
        }
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_search_source_and_operator_names_with_ranked_literal_matching() {
        let pool = get_postgres_client().await;
        let operator_id = uuid::Uuid::now_v7();
        insert_party(&pool, operator_id, "muller-operator", "Müller Kunsthandel").await;
        insert_source(
            &pool,
            uuid::Uuid::now_v7(),
            "muller-source",
            "Müller Auktionshaus",
            operator_id,
        )
        .await;
        insert_source(
            &pool,
            uuid::Uuid::now_v7(),
            "operator-source",
            "Other Source",
            operator_id,
        )
        .await;

        let source_matches = search(&pool, request(Some("muller"), 21, None)).await;
        assert_eq!("Müller Auktionshaus", source_matches.items[0].name.as_ref());
        assert_eq!(2, source_matches.items.len());

        let operator_matches = search(&pool, request(Some("kunst"), 21, None)).await;
        assert_eq!(2, operator_matches.items.len());
        assert!(
            operator_matches
                .items
                .iter()
                .all(|item| item.operator.name.as_ref() == "Müller Kunsthandel")
        );

        let short_prefix = search(&pool, request(Some("au"), 21, None)).await;
        assert!(short_prefix.items.is_empty());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_traverse_more_than_a_thousand_matching_sources_without_duplicates() {
        let pool = get_postgres_client().await;
        let party_id = uuid::Uuid::now_v7();
        insert_party(&pool, party_id, "fanout-operator", "Fanout Operator").await;
        for index in 0..1_001 {
            insert_source(
                &pool,
                uuid::Uuid::now_v7(),
                &format!("catalogue-{index}"),
                &format!("Catalogue Match {index:04}"),
                party_id,
            )
            .await;
        }

        let mut result = search(&pool, request(Some("catalogue"), 50, None)).await;
        assert_eq!(50, result.items.len());
        assert!(result.continuation.is_some());

        let mut seen = std::collections::BTreeSet::new();
        loop {
            for item in result.items {
                assert!(seen.insert(item.listing_source_id));
            }
            let Some(continuation) = result.continuation else {
                break;
            };
            result = search(&pool, request(Some("catalogue"), 50, Some(continuation))).await;
        }
        assert_eq!(1_001, seen.len());
    }

    fn request(
        query: Option<&str>,
        size: u8,
        continuation: Option<listing_source_service::use_cases::queries::search_public_listing_sources::PublicListingSourceSearchContinuation>,
    ) -> SearchPublicListingSourcesRequest {
        SearchPublicListingSourcesRequest::new(
            PublicListingSourceSearchQuery::new(query.map(ToOwned::to_owned))
                .unwrap_or_else(|error| panic!("valid test query: {error}")),
            size,
            continuation,
        )
        .unwrap_or_else(|error| panic!("valid test request: {error}"))
    }

    async fn search(
        pool: &sqlx::PgPool,
        request: SearchPublicListingSourcesRequest,
    ) -> SearchPublicListingSourcesResult {
        SearchPublicListingSourcesHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxPublicListingSourceSearchReaderFactory::new(),
        )
        .execute(
            &OperationContext {
                principal: Principal::Anonymous,
                request_id: RequestId::new("request"),
                correlation_id: CorrelationId::new("correlation"),
            },
            request,
        )
        .await
        .unwrap_or_else(|error| panic!("search public sources: {error}"))
    }

    async fn insert_party(pool: &sqlx::PgPool, id: uuid::Uuid, slug: &str, name: &str) {
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(slug)
            .bind(name)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert test party: {error}"));
    }

    async fn insert_source(
        pool: &sqlx::PgPool,
        id: uuid::Uuid,
        slug: &str,
        name: &str,
        party_id: uuid::Uuid,
    ) {
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(slug)
            .bind(name)
            .bind(party_id)
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert test source: {error}"));
    }
}

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
    use super::super::public_listing_source_details_reader::DETAIL_BY_SLUG_SQL;
    use super::*;
    use crate::SqlxPublicListingSourceDetailsReaderFactory;
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use listing_source_core::ListingSourceSlugId;
    use listing_source_service::use_cases::queries::{
        get_public_listing_source_by_slug::{
            GetPublicListingSourceBySlugHandler, GetPublicListingSourceBySlugRequest,
            GetPublicListingSourceBySlugUseCase,
        },
        search_public_listing_sources::{
            PublicListingSourceSearchQuery, SearchPublicListingSourcesHandler,
            SearchPublicListingSourcesRequest, SearchPublicListingSourcesResult,
            SearchPublicListingSourcesUseCase,
        },
    };
    use platform_postgres::SqlxUnitOfWork;
    use std::time::{Duration, Instant};
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

    const PERFORMANCE_SOURCE_COUNTS: [i64; 3] = [1_000, 10_000, 100_000];
    const PERFORMANCE_SOURCES_PER_OPERATOR: i64 = 20;
    const PERFORMANCE_WARMUP_SAMPLE_COUNT: usize = 3;
    const PERFORMANCE_TIMING_SAMPLE_COUNT: usize = 25;
    const PERFORMANCE_P95_TARGET: Duration = Duration::from_millis(150);

    #[derive(Clone, Copy)]
    struct PerformanceScenario {
        source_count: i64,
        operator_count: i64,
    }

    impl PerformanceScenario {
        fn new(source_count: i64) -> Self {
            Self {
                source_count,
                operator_count: (source_count / PERFORMANCE_SOURCES_PER_OPERATOR).max(1),
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct PerformanceTiming {
        p50: Duration,
        p95: Duration,
    }

    #[derive(Clone, Copy)]
    struct PerformanceTimingResult {
        scenario: PerformanceScenario,
        operation: &'static str,
        timing: PerformanceTiming,
    }

    #[derive(sqlx::FromRow)]
    struct PerformanceCursor {
        listing_source_id: uuid::Uuid,
        name_search: String,
    }

    #[derive(sqlx::FromRow)]
    struct PostgreSqlPerformanceEnvironment {
        server_version: String,
        server_version_num: String,
        server_encoding: String,
        shared_buffers: String,
        effective_cache_size: String,
        work_mem: String,
        random_page_cost: String,
        jit: String,
        database_collation: String,
        database_ctype: String,
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    #[ignore = "opt-in real PostgreSQL public ListingSource search performance harness"]
    async fn should_capture_public_listing_source_search_postgres_performance() {
        let pool = get_postgres_client().await;
        let mut timing_results = Vec::new();
        for source_count in PERFORMANCE_SOURCE_COUNTS {
            timing_results.extend(
                run_performance_scenario(&pool, PerformanceScenario::new(source_count)).await,
            );
        }
        for result in timing_results {
            print_actual_timing_result(result);
        }
    }

    async fn run_performance_scenario(
        pool: &sqlx::PgPool,
        scenario: PerformanceScenario,
    ) -> Vec<PerformanceTimingResult> {
        sqlx::query("TRUNCATE TABLE listing_sources, parties RESTART IDENTITY CASCADE")
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("reset performance scenario: {error}"));
        seed_performance_sources(pool, scenario).await;
        sqlx::query("ANALYZE parties")
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("analyze performance parties: {error}"));
        sqlx::query("ANALYZE listing_sources")
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("analyze performance listing sources: {error}"));

        let source_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM listing_sources")
            .fetch_one(pool)
            .await
            .unwrap_or_else(|error| panic!("count performance listing sources: {error}"));
        let operator_count = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM parties")
            .fetch_one(pool)
            .await
            .unwrap_or_else(|error| panic!("count performance operators: {error}"));
        assert_eq!(scenario.source_count, source_count);
        assert_eq!(scenario.operator_count, operator_count);

        let browse_cursor = performance_cursor(
            pool,
            "SELECT listing_source_id, name_search FROM listing_sources ORDER BY name_search COLLATE \"C\", listing_source_id LIMIT 11",
        )
        .await;
        let prefix_cursor = performance_cursor(
            pool,
            "SELECT listing_source_id, name_search FROM listing_sources WHERE name_search LIKE 'muller%' ORDER BY name_search COLLATE \"C\", listing_source_id LIMIT 11",
        )
        .await;
        let contains_cursor = performance_cursor(
            pool,
            "SELECT listing_source_id, name_search FROM listing_sources WHERE name_search LIKE '%auction%' AND name_search NOT LIKE 'muller%' ORDER BY name_search COLLATE \"C\", listing_source_id LIMIT 11",
        )
        .await;

        let mut transaction = pool
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin performance transaction: {error}"));
        let connection = &mut *transaction;
        print_performance_environment(connection, scenario).await;
        prepare_performance_queries(connection, scenario).await;

        let cases = vec![
            (
                "browse-first-page",
                performance_statement_name("public_listing_source_browse_first", scenario),
                "21".to_owned(),
            ),
            (
                "browse-later-page",
                performance_statement_name("public_listing_source_browse_later", scenario),
                format!(
                    "'{}', '{}', 21",
                    sql_literal(&browse_cursor.name_search),
                    browse_cursor.listing_source_id
                ),
            ),
            (
                "prefix-first-page",
                performance_statement_name("public_listing_source_prefix_first", scenario),
                "'mu', 'mu%', 21".to_owned(),
            ),
            (
                "prefix-later-page",
                performance_statement_name("public_listing_source_prefix_later", scenario),
                format!(
                    "'mu', 'mu%', 1, '{}', '{}', 21",
                    sql_literal(&prefix_cursor.name_search),
                    prefix_cursor.listing_source_id
                ),
            ),
            (
                "contains-first-page",
                performance_statement_name("public_listing_source_contains_first", scenario),
                "'auction', 'auction%', '%auction%', 21".to_owned(),
            ),
            (
                "contains-later-page",
                performance_statement_name("public_listing_source_contains_later", scenario),
                format!(
                    "'auction', 'auction%', '%auction%', 1, '{}', '{}', 21",
                    sql_literal(&contains_cursor.name_search),
                    contains_cursor.listing_source_id
                ),
            ),
            (
                "slug-lookup",
                performance_statement_name("public_listing_source_slug_lookup", scenario),
                "'performance-source-000000'".to_owned(),
            ),
        ];
        capture_plan_mode(connection, scenario, "force_custom_plan", &cases).await;
        capture_plan_mode(connection, scenario, "force_generic_plan", &cases).await;
        transaction
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit performance transaction: {error}"));
        capture_actual_reader_timings(pool, scenario).await
    }

    async fn capture_actual_reader_timings(
        pool: &sqlx::PgPool,
        scenario: PerformanceScenario,
    ) -> Vec<PerformanceTimingResult> {
        let mut results = Vec::new();
        for (operation, timing) in [
            ("browse", measure_search_handler(pool, None).await),
            ("prefix", measure_search_handler(pool, Some("mu")).await),
            (
                "contains",
                measure_search_handler(pool, Some("auction")).await,
            ),
            ("slug", measure_slug_handler(pool).await),
        ] {
            assert!(
                timing.p95 <= PERFORMANCE_P95_TARGET,
                "local p95 target exceeded for {operation} at {} ListingSources: {:.3} ms > {:.3} ms",
                scenario.source_count,
                timing.p95.as_secs_f64() * 1_000.0,
                PERFORMANCE_P95_TARGET.as_secs_f64() * 1_000.0,
            );
            results.push(PerformanceTimingResult {
                scenario,
                operation,
                timing,
            });
        }
        results
    }

    fn print_actual_timing_result(result: PerformanceTimingResult) {
        println!(
            "PUBLIC_LISTING_SOURCE_SEARCH_PERFORMANCE timing scenario_listing_sources={} scenario_operators={} operation={} warmup_samples={} measured_samples={} percentile=nearest-rank p50_ms={:.3} p95_ms={:.3} p95_target_ms={:.3} caveat=warm-local-sequential",
            result.scenario.source_count,
            result.scenario.operator_count,
            result.operation,
            PERFORMANCE_WARMUP_SAMPLE_COUNT,
            PERFORMANCE_TIMING_SAMPLE_COUNT,
            result.timing.p50.as_secs_f64() * 1_000.0,
            result.timing.p95.as_secs_f64() * 1_000.0,
            PERFORMANCE_P95_TARGET.as_secs_f64() * 1_000.0,
        );
    }

    async fn measure_search_handler(pool: &sqlx::PgPool, query: Option<&str>) -> PerformanceTiming {
        let mut samples = Vec::with_capacity(PERFORMANCE_TIMING_SAMPLE_COUNT);
        for sample_index in 0..(PERFORMANCE_WARMUP_SAMPLE_COUNT + PERFORMANCE_TIMING_SAMPLE_COUNT) {
            let started = Instant::now();
            let result = search(pool, request(query, 21, None)).await;
            assert!(
                !result.items.is_empty(),
                "timed search must return fixtures"
            );
            if sample_index >= PERFORMANCE_WARMUP_SAMPLE_COUNT {
                samples.push(started.elapsed());
            }
        }
        summarize_timing(samples)
    }

    async fn measure_slug_handler(pool: &sqlx::PgPool) -> PerformanceTiming {
        let mut samples = Vec::with_capacity(PERFORMANCE_TIMING_SAMPLE_COUNT);
        for sample_index in 0..(PERFORMANCE_WARMUP_SAMPLE_COUNT + PERFORMANCE_TIMING_SAMPLE_COUNT) {
            let started = Instant::now();
            let summary = GetPublicListingSourceBySlugHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxPublicListingSourceDetailsReaderFactory::new(),
            )
            .execute(
                &performance_operation_context(),
                GetPublicListingSourceBySlugRequest {
                    slug_id: ListingSourceSlugId::raw("performance-source-000000")
                        .unwrap_or_else(|error| panic!("valid performance slug: {error}")),
                },
            )
            .await
            .unwrap_or_else(|error| panic!("timed public slug lookup: {error}"));
            assert_eq!(
                "performance-source-000000",
                summary.listing_source_slug_id.as_ref()
            );
            if sample_index >= PERFORMANCE_WARMUP_SAMPLE_COUNT {
                samples.push(started.elapsed());
            }
        }
        summarize_timing(samples)
    }

    fn summarize_timing(mut samples: Vec<Duration>) -> PerformanceTiming {
        assert_eq!(PERFORMANCE_TIMING_SAMPLE_COUNT, samples.len());
        samples.sort_unstable();
        PerformanceTiming {
            p50: samples[nearest_rank_index(samples.len(), 50)],
            p95: samples[nearest_rank_index(samples.len(), 95)],
        }
    }

    fn nearest_rank_index(sample_count: usize, percentile: usize) -> usize {
        ((sample_count * percentile).div_ceil(100)).saturating_sub(1)
    }

    fn performance_operation_context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("performance-request"),
            correlation_id: CorrelationId::new("performance-correlation"),
        }
    }

    async fn seed_performance_sources(pool: &sqlx::PgPool, scenario: PerformanceScenario) {
        sqlx::query(
            "INSERT INTO parties (party_id, party_slug_id, name) SELECT ('00000000-0000-7000-8000-' || lpad(operator_index::text, 12, '0'))::uuid, 'performance-operator-' || lpad(operator_index::text, 6, '0'), CASE WHEN operator_index % 20 = 0 THEN 'Müller Auction Operator ' || operator_index ELSE 'Regional Operator ' || operator_index END FROM generate_series(0, $1) AS operator_index",
        )
        .bind(scenario.operator_count - 1)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed performance operators: {error}"));

        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT ('00000000-0000-7001-8000-' || lpad(source_index::text, 12, '0'))::uuid, 'performance-source-' || lpad(source_index::text, 6, '0'), CASE WHEN source_index % 25 = 0 THEN 'Müller Auction House ' || source_index WHEN source_index % 25 = 1 THEN 'Auction Catalogue ' || source_index ELSE 'Regional Source ' || source_index END, ('00000000-0000-7000-8000-' || lpad((source_index % $1)::text, 12, '0'))::uuid FROM generate_series(0, $2) AS source_index",
        )
        .bind(scenario.operator_count)
        .bind(scenario.source_count - 1)
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("seed performance listing sources: {error}"));
    }

    async fn performance_cursor(pool: &sqlx::PgPool, query: &'static str) -> PerformanceCursor {
        sqlx::query_as::<_, PerformanceCursor>(query)
            .fetch_all(pool)
            .await
            .unwrap_or_else(|error| panic!("select performance cursor rows: {error}"))
            .into_iter()
            .nth(10)
            .unwrap_or_else(|| panic!("performance cursor fixture needs 11 ordered rows"))
    }

    async fn print_performance_environment(
        connection: &mut sqlx::PgConnection,
        scenario: PerformanceScenario,
    ) {
        let environment = sqlx::query_as::<_, PostgreSqlPerformanceEnvironment>(
            "SELECT current_setting('server_version') AS server_version, current_setting('server_version_num') AS server_version_num, current_setting('server_encoding') AS server_encoding, current_setting('shared_buffers') AS shared_buffers, current_setting('effective_cache_size') AS effective_cache_size, current_setting('work_mem') AS work_mem, current_setting('random_page_cost') AS random_page_cost, current_setting('jit') AS jit, database.datcollate AS database_collation, database.datctype AS database_ctype FROM pg_database AS database WHERE database.datname = current_database()",
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap_or_else(|error| panic!("read performance environment: {error}"));
        let image_override = std::env::var("AURA_TEST_POSTGRES_IMAGE")
            .unwrap_or_else(|_| "test-api pinned image (no override)".to_owned());

        println!(
            "PUBLIC_LISTING_SOURCE_SEARCH_PERFORMANCE environment image_override={image_override:?} server_version={:?} server_version_num={} encoding={} database_collation={:?} database_ctype={:?} shared_buffers={} effective_cache_size={} work_mem={} random_page_cost={} jit={} scenario_operators={} scenario_listing_sources={}",
            environment.server_version,
            environment.server_version_num,
            environment.server_encoding,
            environment.database_collation,
            environment.database_ctype,
            environment.shared_buffers,
            environment.effective_cache_size,
            environment.work_mem,
            environment.random_page_cost,
            environment.jit,
            scenario.operator_count,
            scenario.source_count,
        );
    }

    async fn prepare_performance_queries(
        connection: &mut sqlx::PgConnection,
        scenario: PerformanceScenario,
    ) {
        prepare_performance_query(
            connection,
            &performance_statement_name("public_listing_source_browse_first", scenario),
            "bigint",
            &format!("{BROWSE_SQL} ORDER BY s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT $1"),
        )
        .await;
        prepare_performance_query(
            connection,
            &performance_statement_name("public_listing_source_browse_later", scenario),
            "text, uuid, bigint",
            &format!("{BROWSE_SQL} AND (s.name_search COLLATE \"C\" > $1 COLLATE \"C\" OR (s.name_search COLLATE \"C\" = $1 COLLATE \"C\" AND s.listing_source_id > $2)) ORDER BY s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT $3"),
        )
        .await;
        prepare_performance_query(
            connection,
            &performance_statement_name("public_listing_source_prefix_first", scenario),
            "text, text, bigint",
            &format!("WITH input AS (SELECT $1::text AS query, $2{PREFIX_SQL_AFTER_INPUT} ORDER BY b.match_tier ASC, s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT $3"),
        )
        .await;
        prepare_performance_query(
            connection,
            &performance_statement_name("public_listing_source_prefix_later", scenario),
            "text, text, smallint, text, uuid, bigint",
            &format!("WITH input AS (SELECT $1::text AS query, $2{PREFIX_SQL_AFTER_INPUT} AND (b.match_tier > $3 OR (b.match_tier = $3 AND s.name_search COLLATE \"C\" > $4 COLLATE \"C\") OR (b.match_tier = $3 AND s.name_search COLLATE \"C\" = $4 COLLATE \"C\" AND s.listing_source_id > $5)) ORDER BY b.match_tier ASC, s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT $6"),
        )
        .await;
        prepare_performance_query(
            connection,
            &performance_statement_name("public_listing_source_contains_first", scenario),
            "text, text, text, bigint",
            &format!("WITH input AS (SELECT $1::text AS query, $2::text AS prefix_pattern, $3{CONTAINS_SQL_AFTER_INPUT} ORDER BY b.match_tier ASC, s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT $4"),
        )
        .await;
        prepare_performance_query(
            connection,
            &performance_statement_name("public_listing_source_contains_later", scenario),
            "text, text, text, smallint, text, uuid, bigint",
            &format!("WITH input AS (SELECT $1::text AS query, $2::text AS prefix_pattern, $3{CONTAINS_SQL_AFTER_INPUT} AND (b.match_tier > $4 OR (b.match_tier = $4 AND s.name_search COLLATE \"C\" > $5 COLLATE \"C\") OR (b.match_tier = $4 AND s.name_search COLLATE \"C\" = $5 COLLATE \"C\" AND s.listing_source_id > $6)) ORDER BY b.match_tier ASC, s.name_search COLLATE \"C\" ASC, s.listing_source_id ASC LIMIT $7"),
        )
        .await;
        prepare_performance_query(
            connection,
            &performance_statement_name("public_listing_source_slug_lookup", scenario),
            "text",
            DETAIL_BY_SLUG_SQL,
        )
        .await;
    }

    async fn prepare_performance_query(
        connection: &mut sqlx::PgConnection,
        name: &str,
        parameter_types: &str,
        query: &str,
    ) {
        // Names, types, and SQL come only from this fixed harness; no external input reaches it.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "PREPARE {name} ({parameter_types}) AS {query}"
        )))
        .persistent(false)
        .execute(&mut *connection)
        .await
        .unwrap_or_else(|error| panic!("prepare performance query {name}: {error}"));
    }

    async fn capture_plan_mode(
        connection: &mut sqlx::PgConnection,
        scenario: PerformanceScenario,
        mode: &str,
        cases: &[(&str, String, String)],
    ) {
        sqlx::query("SELECT set_config('plan_cache_mode', $1, true)")
            .bind(mode)
            .execute(&mut *connection)
            .await
            .unwrap_or_else(|error| panic!("set performance plan mode {mode}: {error}"));

        for (label, name, arguments) in cases {
            let plan = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(format!(
                "EXPLAIN (ANALYZE, BUFFERS, SETTINGS, FORMAT TEXT) EXECUTE {name}({arguments})"
            )))
            .persistent(false)
            .fetch_all(&mut *connection)
            .await
            .unwrap_or_else(|error| panic!("explain performance {label} in {mode}: {error}"));
            assert!(
                !plan.is_empty(),
                "performance plan must not be empty: {label}"
            );
            println!(
                "PUBLIC_LISTING_SOURCE_SEARCH_PERFORMANCE scenario_listing_sources={} scenario_operators={} case={label} plan_cache_mode={mode}\n{}",
                scenario.source_count,
                scenario.operator_count,
                plan.join("\n")
            );
        }
    }

    fn performance_statement_name(base: &str, scenario: PerformanceScenario) -> String {
        format!("{base}_{}", scenario.source_count)
    }

    fn sql_literal(value: &str) -> String {
        value.replace('\'', "''")
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

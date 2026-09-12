use application::transaction::{Transaction, UnitOfWork};
use aura_historia_worker::search_filter_projection::consume_search_filter_projection_queue;
use aura_historia_worker::{WorkerRunError, WorkerScope, serve_with_runtime};
use domain_primitives::event_id::EventId;
use localization::{Language, Localized};
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use product_listing_core::{
    product_listing::ProductListingPricing, source_listing_id::SourceListingId, title::Title,
};
use product_listing_service::ports::{
    ListingSourceSummary, ProductListingPercolationInput, ProductListingSearchFilterMatchSource,
};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use search_filter_core::{NewSearchFilter, SearchFilter};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::{SqlxSearchFilterIndexReader, SqlxSearchFilterRepositoryFactory};
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterIndexQuery, SearchFilterRepository,
    SearchFilterRepositoryFactory,
};
use search_filter_service::use_cases::{
    ProjectSearchFilterChangeHandler, ProjectSearchFilterChangeUseCase,
};

use std::sync::Arc;
use std::time::{Duration, Instant};
use test_api::{
    IntegrationTestService, OpenSearch, Postgres, Sequin, aura_integration_test,
    get_opensearch_client, get_postgres_client, get_sequin_worker_webhook_bind_addr, refresh_index,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
mod support;
const SCOPE: WorkerScope = WorkerScope::SearchFilterProjection;
const WORKER_SQS: test_api::WorkerSqs = support::queues(SCOPE);
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook_for_tables(&["public.search_filters"]);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const POLL_ATTEMPTS: usize = 120;
const ROLLBACK_OBSERVATION_DURATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_project_search_filter_insert_from_sequin() {
    let result = project_search_filter_insert_from_sequin().await;

    assert!(
        result.is_ok(),
        "Sequin search-filter insert projection acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_replace_search_filter_projection_after_sequin_update() {
    let result = project_search_filter_update_from_sequin().await;

    assert!(
        result.is_ok(),
        "Sequin search-filter update projection acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_project_rolled_back_search_filter_insert_from_sequin() {
    let result = reject_rolled_back_search_filter_insert_from_sequin().await;

    assert!(
        result.is_ok(),
        "Sequin rolled-back search-filter projection acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_remove_search_filter_projection_after_sequin_delete() {
    let result = project_search_filter_delete_from_sequin().await;

    assert!(
        result.is_ok(),
        "Sequin search-filter delete projection acceptance test failed: {result:?}"
    );
}

async fn project_search_filter_insert_from_sequin() -> Result<(), Box<dyn std::error::Error>> {
    let worker = ProjectionWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool).await?;
        let filter = search_filter(user_id, "Sequin insert cabinet")?;

        let version = insert_filter(&worker.pool, &filter).await?;

        wait_for_percolation(&worker.index, filter.id(), "Sequin insert cabinet", true).await?;
        assert_eq!(1, version);
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn project_search_filter_update_from_sequin() -> Result<(), Box<dyn std::error::Error>> {
    let worker = ProjectionWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool).await?;
        let mut filter = search_filter(user_id, "Sequin original cabinet")?;
        let inserted_version = insert_filter(&worker.pool, &filter).await?;
        wait_for_percolation(&worker.index, filter.id(), "Sequin original cabinet", true).await?;

        filter.replace_search(
            ProductListingSearch::new(Language::En, Currency::Eur)
                .with_product_listing_query("Sequin replacement cabinet".try_into()?),
            None,
        );
        let updated_version = update_filter(&worker.pool, &filter, inserted_version).await?;

        wait_for_percolation(
            &worker.index,
            filter.id(),
            "Sequin replacement cabinet",
            true,
        )
        .await?;
        wait_for_percolation(&worker.index, filter.id(), "Sequin original cabinet", false).await?;
        assert_eq!(2, updated_version);
        for (version, operation) in [(2, "update"), (1, "insert"), (2, "update"), (1, "insert")] {
            redeliver_filter(user_id, filter.id(), version, operation).await?;
        }
        support::wait_until_empty(SCOPE).await?;
        wait_for_percolation(
            &worker.index,
            filter.id(),
            "Sequin replacement cabinet",
            true,
        )
        .await?;
        wait_for_percolation(&worker.index, filter.id(), "Sequin original cabinet", false).await?;
        let document: serde_json::Value = get_opensearch_client()
            .await
            .get(opensearch::GetParts::IndexId(
                "user_search_filters",
                &filter.id().to_string(),
            ))
            .send()
            .await?
            .error_for_status_code()?
            .json()
            .await?;
        assert_eq!(serde_json::json!(2), document["_version"]);
        assert_eq!(serde_json::json!(2), document["_source"]["sourceVersion"]);
        let query = worker
            .index
            .query(&search_filter_service::ports::SearchFilterIndexQuery::default())
            .await?;
        assert_eq!(1, query.items.len());
        assert_eq!(Some(1), query.total);
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn reject_rolled_back_search_filter_insert_from_sequin()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = ProjectionWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool).await?;
        let filter = search_filter(user_id, "Sequin rolled-back cabinet")?;

        insert_filter_then_rollback(&worker.pool, &filter).await?;
        assert_filter_is_not_persisted(&worker.pool, filter.id()).await?;

        assert_not_percolated_for(
            &worker.index,
            filter.id(),
            "Sequin rolled-back cabinet",
            ROLLBACK_OBSERVATION_DURATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn project_search_filter_delete_from_sequin() -> Result<(), Box<dyn std::error::Error>> {
    let worker = ProjectionWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool).await?;
        let filter = search_filter(user_id, "Sequin deleted cabinet")?;

        insert_filter(&worker.pool, &filter).await?;
        wait_for_percolation(&worker.index, filter.id(), "Sequin deleted cabinet", true).await?;

        delete_filter(&worker.pool, filter.id()).await?;

        wait_for_percolation(&worker.index, filter.id(), "Sequin deleted cabinet", false).await?;
        support::wait_until_empty(SCOPE).await?;
        assert_filter_tombstone(&worker.index, filter.id(), 2).await?;
        for (operation, version) in [("delete", 1), ("insert", 1), ("delete", 1), ("update", 1)] {
            redeliver_filter(user_id, filter.id(), version, operation).await?;
        }
        support::wait_until_empty(SCOPE).await?;
        assert_filter_tombstone(&worker.index, filter.id(), 2).await?;
        Ok(())
    }
    .await;

    worker.finish(result).await
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_project_inactive_filter_but_hide_it_from_active_queries_and_reject_unrouted_cdc() {
    let result: support::TestResult = async {
        let worker = ProjectionWorker::start().await?;
        let result = async {
            let user = seed_user(&worker.pool).await?;
            let filter = search_filter(user, "Inactive SQS filter cabinet")?;
            insert_filter(&worker.pool, &filter).await?;
            wait_for_percolation(&worker.index, filter.id(), "Inactive SQS filter cabinet", true).await?;
            sqlx::query("UPDATE search_filters SET state = 'INACTIVE_BY_USER', version = version + 1 WHERE user_search_filter_id = $1")
                .bind(filter.id().as_uuid()).execute(&worker.pool).await?;
            wait_for_filter_state(&worker.index, filter.id(), SearchFilterState::InactiveByUser).await?;
            let active = worker.index.query(&SearchFilterIndexQuery {
                state: Some(SearchFilterState::Active),
                ..Default::default()
            }).await?;
            assert!(active.items.is_empty());
            assert_eq!(Some(0), active.total);
            let document: serde_json::Value = get_opensearch_client().await
                .get(opensearch::GetParts::IndexId("user_search_filters", &filter.id().to_string()))
                .send().await?.error_for_status_code()?.json().await?;
            assert_eq!(serde_json::json!(2), document["_version"]);
            assert_eq!(serde_json::json!("INACTIVE_BY_USER"), document["_source"]["state"]);
            assert_eq!(1, worker.index.query(&search_filter_service::ports::SearchFilterIndexQuery::default()).await?.items.len());
            let response = reqwest::Client::new()
                .post(format!("http://127.0.0.1:{}/cdc/sequin", get_sequin_worker_webhook_bind_addr().port()))
                .json(&serde_json::json!({"changes": [{"table": "users", "operation": "update", "record": {"user_id": user.as_uuid().to_string()}}]}))
                .send().await?;
            assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, response.status());
            support::wait_until_empty(SCOPE).await
        }.await;
        worker.finish(result).await
    }.await;
    result.expect("inactive/unrouted filter acceptance and cleanup");
}

async fn redeliver_filter(
    user_id: UserId,
    id: UserSearchFilterId,
    version: i64,
    operation: &str,
) -> support::TestResult {
    support::post_change(serde_json::json!({
        "record": {
            "user_id": user_id.as_uuid().to_string(),
            "user_search_filter_id": id.as_uuid().to_string(),
            "version": version
        },
        "action": operation,
        "metadata": {"table_schema": "public", "table_name": "search_filters"},
    }))
    .await
}

async fn assert_filter_tombstone(
    index: &OpenSearchSearchFilterIndex,
    id: UserSearchFilterId,
    version: i64,
) -> support::TestResult {
    let document: serde_json::Value = get_opensearch_client()
        .await
        .get(opensearch::GetParts::IndexId(
            "user_search_filters",
            &id.to_string(),
        ))
        .send()
        .await?
        .error_for_status_code()?
        .json()
        .await?;
    assert_eq!(serde_json::json!(version), document["_version"]);
    assert_eq!(
        serde_json::json!({"userSearchFilterId": id, "sourceVersion": version, "projectionDeleted": true}),
        document["_source"]
    );
    refresh_index("user_search_filters").await;
    let query = index
        .query(&search_filter_service::ports::SearchFilterIndexQuery::default())
        .await?;
    assert!(query.items.is_empty());
    assert_eq!(Some(0), query.total);
    wait_for_percolation(index, id, "Sequin deleted cabinet", false).await
}

struct ProjectionWorker {
    pool: sqlx::PgPool,
    index: OpenSearchSearchFilterIndex,
    projection_task: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl ProjectionWorker {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        let index = OpenSearchSearchFilterIndex::new(get_opensearch_client().await.clone());
        let handler: Arc<dyn ProjectSearchFilterChangeUseCase> =
            Arc::new(ProjectSearchFilterChangeHandler::new(
                SqlxSearchFilterIndexReader::new(pool.clone()),
                index.clone(),
            ));
        let (runtime, receiver) = support::composition(SCOPE).await?.into_parts();
        let projection_task = support::competing_consumers(SCOPE, receiver, move |receiver| {
            consume_search_filter_projection_queue(receiver, handler.clone())
        })
        .await?;
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr()).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        Ok(Self {
            pool,
            index,
            projection_task,
            shutdown_tx,
            server,
        })
    }

    async fn finish(
        self,
        test_result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let shutdown_result = self.shutdown().await;

        test_result?;
        shutdown_result
    }

    async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        self.shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker server shutdown channel closed"))?;
        self.server.await??;
        self.projection_task.await?;
        Ok(())
    }
}

async fn insert_filter(
    pool: &sqlx::PgPool,
    filter: &SearchFilter,
) -> Result<i64, Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let inserted = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut transaction)
        .insert(filter)
        .await?;
    transaction.commit().await?;
    Ok(inserted.version)
}

async fn insert_filter_then_rollback(
    pool: &sqlx::PgPool,
    filter: &SearchFilter,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut transaction)
        .insert(filter)
        .await?;
    drop(transaction);
    Ok(())
}

async fn update_filter(
    pool: &sqlx::PgPool,
    filter: &SearchFilter,
    expected_version: i64,
) -> Result<i64, Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    let updated = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut transaction)
        .update(filter, expected_version)
        .await?;
    transaction.commit().await?;
    Ok(updated.version)
}

async fn delete_filter(
    pool: &sqlx::PgPool,
    filter_id: UserSearchFilterId,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut transaction)
        .delete(filter_id)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn assert_filter_is_not_persisted(
    pool: &sqlx::PgPool,
    filter_id: UserSearchFilterId,
) -> Result<(), Box<dyn std::error::Error>> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM search_filters WHERE user_search_filter_id = $1)",
    )
    .bind(filter_id.as_uuid())
    .fetch_one(pool)
    .await?;

    assert!(
        !exists,
        "rolled-back search filter {filter_id} was persisted"
    );
    Ok(())
}

async fn assert_not_percolated_for(
    index: &OpenSearchSearchFilterIndex,
    filter_id: UserSearchFilterId,
    title: &str,
    observation_duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + observation_duration;

    loop {
        refresh_index("user_search_filters").await;
        let product = ProductListingPercolationInput {
            source: product_source(title)?,
            valuation: None,
        };
        let matches = index.percolate(&product).await?;
        if matches
            .iter()
            .any(|search_filter| search_filter.search_filter_id == filter_id)
        {
            return Err(std::io::Error::other(format!(
                "rolled-back search filter {filter_id} was projected for {title:?}"
            ))
            .into());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_filter_state(
    index: &OpenSearchSearchFilterIndex,
    id: UserSearchFilterId,
    state: SearchFilterState,
) -> support::TestResult {
    let query = SearchFilterIndexQuery {
        state: Some(state),
        ..Default::default()
    };
    for _ in 0..POLL_ATTEMPTS {
        refresh_index("user_search_filters").await;
        if index
            .query(&query)
            .await?
            .items
            .iter()
            .any(|filter| filter.search_filter_id == id)
        {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(format!("search filter {id} did not reach projected state {state:?}").into())
}

async fn wait_for_percolation(
    index: &OpenSearchSearchFilterIndex,
    filter_id: UserSearchFilterId,
    title: &str,
    should_match: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        refresh_index("user_search_filters").await;
        let product = ProductListingPercolationInput {
            source: product_source(title)?,
            valuation: None,
        };
        let matches = index.percolate(&product).await?;
        let found = matches
            .iter()
            .any(|search_filter| search_filter.search_filter_id == filter_id);
        if found == should_match {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    Err(std::io::Error::other(format!(
        "search filter {filter_id} did not reach expected percolation state {should_match} for {title:?}"
    ))
    .into())
}

fn product_source(
    title: &str,
) -> Result<ProductListingSearchFilterMatchSource, Box<dyn std::error::Error>> {
    let title = Title::from(title);
    let url = url::Url::parse("https://shop.example.test/product_listings/test-product")?;
    let event_id = EventId::new();

    Ok(ProductListingSearchFilterMatchSource {
        event_id,
        event_kind:
            product_listing_service::ports::ProductListingSearchFilterMatchSourceEventKind::Domain,
        origin_event_time: time::OffsetDateTime::UNIX_EPOCH,
        current_event_id: event_id,
        projection_version: 1,
        product_listing_id: ProductListingId::new(),
        product_listing_title_slug_id: ProductListingSlugId::raw("test-product-a1b2c3")
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
        source: ListingSourceSummary {
            listing_source_id: ListingSourceId::new(),
            name: ListingSourceName::try_from("Test source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            slug_id: ListingSourceSlugId::raw("test-source")
                .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
        },
        source_listing_id: SourceListingId::try_from("test-product-1")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        product_title: Some(Localized::new(Language::En, title.clone())),
        product_description: None,
        titles: std::collections::HashMap::from([(Language::En, title)]),
        descriptions: std::collections::HashMap::new(),
        pricing: ProductListingPricing::default(),
        sale_observation: None,
        availability: Some(ListingAvailability::Available),
        lifecycle: ListingLifecycle::Active,
        url: url.clone(),
        view_url: url,
        image: None,
        images: indexmap::IndexSet::new(),
        embedding: None,
        auction: None,
        created: time::OffsetDateTime::UNIX_EPOCH,
        updated: time::OffsetDateTime::UNIX_EPOCH,
    })
}

fn search_filter(user_id: UserId, query: &str) -> Result<SearchFilter, Box<dyn std::error::Error>> {
    Ok(SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name: UserSearchFilterName::from("Sequin acceptance filter"),
        notifications: true,
        state: SearchFilterState::Active,
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query(query.try_into()?),
        embedding: None,
    }))
}

async fn seed_user(pool: &sqlx::PgPool) -> Result<UserId, sqlx::Error> {
    let user_id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(user_id.as_uuid())
    .bind(format!("sequin-worker-acceptance-{user_id}@example.com"))
    .execute(pool)
    .await?;
    Ok(user_id)
}

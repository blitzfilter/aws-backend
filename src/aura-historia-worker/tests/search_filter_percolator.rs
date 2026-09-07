use application::{
    error::box_error,
    transaction::{Transaction, UnitOfWork},
};
use aura_historia_worker::cdc::WorkerQueue;
use aura_historia_worker::search_filter_match_notifications::consume_search_filter_match_notification_queue;
use aura_historia_worker::search_filter_percolator::consume_search_filter_percolator_queue;
use aura_historia_worker::{QueueConfig, WorkerRunError, WorkerRuntime, serve_with_runtime};
use domain_primitives::{event_id::EventId, query::range_query::RangeQuery};
use fxrate_core::FxRateId;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use large_language_model::{
    LargeLanguageModel, LargeLanguageModelError, StructuredGenerationRequest,
};
use localization::Language;
use money::{Currency, MonetaryAmount};
use notification_postgres::{
    SqlxNotificationDeliveryIntentRepositoryFactory, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use opensearch::GetParts;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_search::ProductListingSearch,
};
use product_listing_postgres::{
    SqlxProductListingContentAssessmentSnapshotReaderFactory,
    SqlxProductListingCurrentEventGuardFactory,
    SqlxProductListingSearchFilterMatchSourceReaderFactory,
};
use search_filter_core::{
    NewSearchFilter, SearchFilter, search_filter_state::SearchFilterState,
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use user_core::user_id::UserId;

use product_listing_service::ports::{
    ProductListingSearchFilterMatchSourceReader, ProductListingSearchFilterMatchSourceReaderFactory,
};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::{
    SqlxActiveSearchFilterMatchCandidateReaderFactory, SqlxSearchFilterIndexReader,
    SqlxSearchFilterMatchNotificationSourceReaderFactory, SqlxSearchFilterMatchWriterFactory,
    SqlxSearchFilterMonthlyMatchQuotaReaderFactory, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::ports::{SearchFilterRepository, SearchFilterRepositoryFactory};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationCommand, GenerateSearchFilterMatchNotificationHandler,
    GenerateSearchFilterMatchNotificationResult, GenerateSearchFilterMatchNotificationUseCase,
    MatchProductListingEventHandler, MatchProductListingEventUseCase,
    ProjectSearchFilterChangeCommand, ProjectSearchFilterChangeHandler,
    ProjectSearchFilterChangeUseCase, SearchFilterProjectionOperation,
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use test_api::{
    IntegrationTestService, OpenSearch, Postgres, Sequin, aura_integration_test,
    get_opensearch_client, get_postgres_client, get_sequin_worker_webhook_bind_addr, refresh_index,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use user_postgres::SqlxUserTierEntitlementsFactory;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_SIDE_EFFECT_OBSERVATION: Duration = Duration::from_secs(2);

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_suppress_search_filter_match_notification_when_withdrawal_commits_first() {
    let pool = get_postgres_client().await;
    let user_id = seed_user(&pool, "ULTIMATE")
        .await
        .unwrap_or_else(|error| panic!("seed user: {error}"));
    let filter = search_filter(
        user_id,
        UserSearchFilterName::from("Withdrawn match notification filter"),
        SearchFilterState::Active,
        "Withdrawn match notification product",
    )
    .unwrap_or_else(|error| panic!("create filter: {error}"));
    insert_filter(&pool, &filter)
        .await
        .unwrap_or_else(|error| panic!("seed filter: {error}"));
    let (product_listing_id, event_id) =
        create_product_with_domain_event(&pool, "Withdrawn match notification product")
            .await
            .unwrap_or_else(|error| panic!("seed product event: {error}"));
    insert_historical_search_filter_match(
        &pool,
        user_id,
        filter.id(),
        product_listing_id,
        event_id,
    )
    .await
    .unwrap_or_else(|error| panic!("seed historical match: {error}"));
    sqlx::query(
        "UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("withdraw product listing: {error}"));

    let handler = GenerateSearchFilterMatchNotificationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSearchFilterMatchNotificationSourceReaderFactory,
        SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
        SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory::new(),
        SqlxProductListingContentAssessmentSnapshotReaderFactory::new(),
        NotificationCreationCoordinatorFactory::new(
            SqlxNotificationRepositoryFactory::new(),
            InitialExternalDeliveryPlanReaderFactory,
            SqlxNotificationDeliveryIntentRepositoryFactory::new(),
        ),
    );
    let outcome = handler
        .execute(GenerateSearchFilterMatchNotificationCommand {
            user_id,
            search_filter_id: filter.id(),
            product_listing_id,
            origin_event_id: event_id,
        })
        .await
        .unwrap_or_else(|error| panic!("generate match notification: {error}"));
    assert_eq!(
        GenerateSearchFilterMatchNotificationResult::SuppressedForWithdrawnProductListing,
        outcome
    );
    let notification_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notifications WHERE user_id = $1 AND product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("count notifications: {error}"));
    assert_eq!(0, notification_count);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_lock_product_listing_through_search_filter_notification_source_read() {
    let pool = get_postgres_client().await;
    let (product_listing_id, event_id) =
        create_product_with_domain_event(&pool, "Search-filter notification lock product")
            .await
            .unwrap_or_else(|error| panic!("seed product event: {error}"));

    let mut notification_transaction = SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin notification transaction: {error}"));
    let source = SqlxProductListingSearchFilterMatchSourceReaderFactory::new()
        .in_transaction(&mut notification_transaction)
        .find_source(event_id, product_listing_id)
        .await
        .unwrap_or_else(|error| panic!("read notification source: {error}"));
    assert!(source.is_some());

    let mut withdrawal_transaction = SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin withdrawal transaction: {error}"));
    let blocked = sqlx::query(
        "SELECT product_listing_id FROM product_listings WHERE product_listing_id = $1 FOR UPDATE NOWAIT",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(withdrawal_transaction.connection())
    .await;
    assert!(matches!(
        blocked,
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03")
    ));

    notification_transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit notification transaction: {error}"));
    drop(withdrawal_transaction);

    let mut withdrawal_transaction = SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin withdrawal transaction after notification: {error}"));
    sqlx::query(
        "UPDATE product_listings SET lifecycle = 'WITHDRAWN', availability = NULL WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(withdrawal_transaction.connection())
    .await
    .unwrap_or_else(|error| panic!("withdraw product listing after notification: {error}"));
    withdrawal_transaction
        .commit()
        .await
        .unwrap_or_else(|error| panic!("commit withdrawal transaction: {error}"));
    let lifecycle: String =
        sqlx::query_scalar("SELECT lifecycle FROM product_listings WHERE product_listing_id = $1")
            .bind(uuid::Uuid::from(product_listing_id))
            .fetch_one(&pool)
            .await
            .unwrap_or_else(|error| panic!("read withdrawn lifecycle: {error}"));
    assert_eq!("WITHDRAWN", lifecycle);
}

struct NonMatchingLargeLanguageModel;

#[async_trait::async_trait]
impl LargeLanguageModel for NonMatchingLargeLanguageModel {
    async fn generate<Output>(
        &self,
        _request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: serde::de::DeserializeOwned + Send,
    {
        serde_json::from_str(r#"{"matches":false}"#).map_err(|source| {
            LargeLanguageModelError::InvalidResponse {
                source: box_error(source),
            }
        })
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_create_notifications_for_committed_product_create_and_update_events() {
    let result = committed_product_create_and_update_flow().await;

    assert!(
        result.is_ok(),
        "search-filter full create/update flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_match_only_active_filter_when_other_filters_are_inactive_or_do_not_match() {
    let result = active_inactive_and_no_match_flow().await;

    assert!(
        result.is_ok(),
        "search-filter filtering flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_percolate_and_persist_all_active_filters_across_pages() {
    let result = complete_percolation_flow().await;

    assert!(
        result.is_ok(),
        "search-filter complete percolation flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_suppress_notification_after_free_tier_monthly_quota() {
    let result = quota_flow().await;

    assert!(
        result.is_ok(),
        "search-filter quota flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_percolate_current_lifecycle_changed_product_listing_event() {
    let result = lifecycle_changed_product_listing_event_flow().await;

    assert!(
        result.is_ok(),
        "search-filter lifecycle changed-event flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_not_process_rolled_back_product_event() {
    let result = rolled_back_product_event_flow().await;

    assert!(
        result.is_ok(),
        "search-filter rollback flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_not_create_matches_for_committed_current_withdrawn_product_listing_event() {
    let result = withdrawn_product_listing_event_flow().await;

    assert!(
        result.is_ok(),
        "search-filter withdrawn product-listing event acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_keep_one_notification_per_matching_filter_on_product_and_match_redelivery() {
    let result = redelivery_and_deterministic_selection_flow().await;

    assert!(
        result.is_ok(),
        "search-filter redelivery and deterministic notification flow acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_keep_current_enrichment_match_for_each_product_event_delivery_order() {
    let result = stale_event_ordering_flow().await;

    assert!(
        result.is_ok(),
        "search-filter stale-event ordering acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OpenSearch(), WORKER_SEQUIN])]
async fn should_percolate_cross_currency_saved_filters_with_event_and_sale_fx_provenance() {
    let result = cross_currency_saved_filter_percolation_flow().await;

    assert!(
        result.is_ok(),
        "cross-currency search-filter percolation acceptance test failed: {result:?}"
    );
}

async fn committed_product_create_and_update_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Worker percolator product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Create and update notification filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;

        let (created_product_listing_id, created_event_id) =
            create_product_with_domain_event(&worker.pool, &product_listing_query).await?;
        assert_eq!(
            "PRODUCT_LISTING_DISCOVERED",
            product_event_type(&worker.pool, created_event_id).await?
        );
        wait_for_match(&worker.pool, created_event_id, 1).await?;
        assert_match_for_event(
            &worker.pool,
            created_event_id,
            user_id,
            filter.id(),
            created_product_listing_id,
        )
        .await?;
        let created_notifications = wait_for_notifications(&worker.pool, user_id, 1).await?;
        assert_search_filter_notification(
            &created_notifications[0],
            user_id,
            filter.id(),
            created_product_listing_id,
            created_event_id,
        )?;

        let update_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Update notification filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        worker.project_filter(&update_filter).await?;
        refresh_index("user_search_filters").await;

        let updated_event_id = update_product_and_insert_event(
            &worker.pool,
            created_product_listing_id,
            &product_listing_query,
        )
        .await?;
        assert_eq!(
            "PRODUCT_LISTING_CHANGED",
            product_event_type(&worker.pool, updated_event_id).await?
        );
        wait_for_match(&worker.pool, updated_event_id, 1).await?;
        assert_match_for_event(
            &worker.pool,
            updated_event_id,
            user_id,
            update_filter.id(),
            created_product_listing_id,
        )
        .await?;
        let notifications = wait_for_notifications(&worker.pool, user_id, 2).await?;
        let created_notification = notifications
            .iter()
            .find(|notification| notification.origin_event_id == uuid::Uuid::from(created_event_id))
            .ok_or_else(|| std::io::Error::other("created product notification is missing"))?;
        assert_search_filter_notification(
            created_notification,
            user_id,
            filter.id(),
            created_product_listing_id,
            created_event_id,
        )?;
        let updated_notification = notifications
            .iter()
            .find(|notification| notification.origin_event_id == uuid::Uuid::from(updated_event_id))
            .ok_or_else(|| std::io::Error::other("updated product notification is missing"))?;
        assert_search_filter_notification(
            updated_notification,
            user_id,
            update_filter.id(),
            created_product_listing_id,
            updated_event_id,
        )?;
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn active_inactive_and_no_match_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Worker percolator product {user_id}");
        let active_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Active matching filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        let inactive_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Inactive matching filter"),
            SearchFilterState::InactiveByUser,
            &product_listing_query,
        )?;
        let no_match_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Active non-matching filter"),
            SearchFilterState::Active,
            "Unrelated carved marble lion",
        )?;
        worker.project_filter(&active_filter).await?;
        worker.project_filter(&inactive_filter).await?;
        worker.project_filter(&no_match_filter).await?;
        refresh_index("user_search_filters").await;

        let (product_listing_id, event_id) =
            create_product_with_domain_event(&worker.pool, &product_listing_query).await?;

        wait_for_match(&worker.pool, event_id, 1).await?;
        let matches = matches_for_event(&worker.pool, event_id).await?;
        assert_eq!(vec![active_filter.id()], matches);
        let notifications = wait_for_notifications(&worker.pool, user_id, 1).await?;
        assert_search_filter_notification(
            &notifications[0],
            user_id,
            active_filter.id(),
            product_listing_id,
            event_id,
        )?;
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn complete_percolation_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = PercolatorWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Worker complete percolation product {user_id}");
        let active_filters: Vec<_> = (0..25)
            .map(|number| {
                search_filter(
                    user_id,
                    UserSearchFilterName::from(format!("Active percolation filter {number}")),
                    SearchFilterState::Active,
                    &product_listing_query,
                )
            })
            .collect::<Result<_, _>>()?;
        let inactive_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Inactive percolation filter"),
            SearchFilterState::InactiveByUser,
            &product_listing_query,
        )?;

        for filter in &active_filters {
            worker.project_filter(filter).await?;
        }
        worker.project_filter(&inactive_filter).await?;
        refresh_index("user_search_filters").await;

        let (_, event_id) =
            create_product_with_domain_event(&worker.pool, &product_listing_query).await?;
        wait_for_match(&worker.pool, event_id, active_filters.len() as i64).await?;

        let mut expected_ids: Vec<_> = active_filters.iter().map(SearchFilter::id).collect();
        expected_ids.sort_by_key(ToString::to_string);
        let persisted_ids = matches_for_event(&worker.pool, event_id).await?;
        assert_eq!(expected_ids, persisted_ids);
        assert!(!persisted_ids.contains(&inactive_filter.id()));
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn quota_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "FREE").await?;
        let product_listing_query = format!("Worker percolator product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Free tier quota filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        let filter_version = insert_filter(&worker.pool, &filter).await?;

        for _ in 0..10 {
            let (product_listing_id, event_id) =
                create_product_with_domain_event(&worker.pool, "Historical quota product").await?;
            insert_historical_search_filter_match(
                &worker.pool,
                user_id,
                filter.id(),
                product_listing_id,
                event_id,
            )
            .await?;
        }
        let _historical_notifications = wait_for_notifications(&worker.pool, user_id, 10).await?;

        worker
            .project_existing_filter(&filter, filter_version)
            .await?;
        refresh_index("user_search_filters").await;

        let (_, event_id) =
            create_product_with_domain_event(&worker.pool, &product_listing_query).await?;

        wait_for_match(&worker.pool, event_id, 1).await?;
        assert_no_more_than_notifications(&worker.pool, user_id, 10, NO_SIDE_EFFECT_OBSERVATION)
            .await
    }
    .await;

    worker.finish(result).await
}

async fn lifecycle_changed_product_listing_event_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Worker percolator product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Lifecycle changed-event filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;

        let (_, lifecycle_event) = create_product_with_event(
            &worker.pool,
            &product_listing_query,
            "PRODUCT_LISTING_CHANGED",
            "DOMAIN",
            json!({"lifecycle": {"transition": "RESTORED"}}),
        )
        .await?;

        wait_for_match(&worker.pool, lifecycle_event, 1).await?;
        assert_no_more_than_notifications(&worker.pool, user_id, 1, NO_SIDE_EFFECT_OBSERVATION)
            .await
    }
    .await;

    worker.finish(result).await
}

async fn rolled_back_product_event_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Worker percolator product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Rollback filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;

        let event_id =
            create_product_with_event_then_rollback(&worker.pool, &product_listing_query).await?;

        assert_event_is_not_persisted(&worker.pool, event_id).await?;
        assert_no_matches_for(&worker.pool, event_id, NO_SIDE_EFFECT_OBSERVATION).await?;
        assert_no_more_than_notifications(&worker.pool, user_id, 0, NO_SIDE_EFFECT_OBSERVATION)
            .await
    }
    .await;

    worker.finish(result).await
}

async fn withdrawn_product_listing_event_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = PercolatorWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Worker withdrawn product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Withdrawn product filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;

        let (product_listing_id, historical_event_id) =
            create_product_with_domain_event(&worker.pool, &product_listing_query).await?;
        wait_for_match(&worker.pool, historical_event_id, 1).await?;

        let withdrawal_event_id =
            withdraw_product_listing_and_insert_event(&worker.pool, product_listing_id).await?;
        assert_product_listing_is_current_and_withdrawn(
            &worker.pool,
            product_listing_id,
            withdrawal_event_id,
        )
        .await?;
        assert_match_count_for_duration(
            &worker.pool,
            historical_event_id,
            1,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await?;
        assert_no_matches_for(
            &worker.pool,
            withdrawal_event_id,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await
    }
    .await;

    worker.finish(result).await
}

async fn stale_event_ordering_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = PercolatorWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Worker stale event product {user_id}");
        let filter = search_filter(
            user_id,
            UserSearchFilterName::from("Stale event ordering filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        worker.project_filter(&filter).await?;
        refresh_index("user_search_filters").await;

        let (product_listing_id, event_a) =
            create_product_with_domain_event(&worker.pool, &product_listing_query).await?;
        let event_b = update_product_and_insert_event_with_group(
            &worker.pool,
            product_listing_id,
            &product_listing_query,
            "ENRICHMENT_EMBEDDED",
            "ENRICHMENT",
        )
        .await?;

        redeliver_product_event(
            &worker.server,
            product_listing_id,
            event_a,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
        )
        .await?;
        redeliver_product_event(
            &worker.server,
            product_listing_id,
            event_b,
            "ENRICHMENT_EMBEDDED",
            "ENRICHMENT",
        )
        .await?;
        wait_for_match(&worker.pool, event_b, 1).await?;
        assert_no_matches_for(&worker.pool, event_a, NO_SIDE_EFFECT_OBSERVATION).await?;

        redeliver_product_event(
            &worker.server,
            product_listing_id,
            event_b,
            "ENRICHMENT_EMBEDDED",
            "ENRICHMENT",
        )
        .await?;
        redeliver_product_event(
            &worker.server,
            product_listing_id,
            event_a,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
        )
        .await?;
        assert_match_count_for_duration(&worker.pool, event_b, 1, NO_SIDE_EFFECT_OBSERVATION)
            .await?;
        assert_no_matches_for(&worker.pool, event_a, NO_SIDE_EFFECT_OBSERVATION).await?;
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn cross_currency_saved_filter_percolation_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = PercolatorWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Cross currency saved-filter product {user_id}");
        let eur_filter = price_search_filter(
            user_id,
            UserSearchFilterName::from("EUR saved-filter bounds"),
            &product_listing_query,
            Currency::Eur,
            9_000,
            13_000,
        )?;
        let usd_filter = price_search_filter(
            user_id,
            UserSearchFilterName::from("USD saved-filter bounds"),
            &product_listing_query,
            Currency::Usd,
            8_500,
            16_000,
        )?;
        let jpy_filter = price_search_filter(
            user_id,
            UserSearchFilterName::from("JPY saved-filter bounds"),
            &product_listing_query,
            Currency::Jpy,
            10_000,
            21_000,
        )?;
        let no_price_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Saved-filter without price bounds"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        let price_filters = [&eur_filter, &usd_filter, &jpy_filter];
        let all_filters = [&eur_filter, &usd_filter, &jpy_filter, &no_price_filter];

        for filter in all_filters {
            worker.project_filter(filter).await?;
        }
        refresh_index("user_search_filters").await;
        assert_price_filter_document(&eur_filter, Currency::Eur, 9_000, 13_000).await?;
        assert_price_filter_document(&usd_filter, Currency::Usd, 8_500, 16_000).await?;
        assert_price_filter_document(&jpy_filter, Currency::Jpy, 10_000, 21_000).await?;

        let snapshot_a_time = time::OffsetDateTime::from_unix_timestamp(1_900_000_000)?;
        let snapshot_a = insert_fx_snapshot(
            &worker.pool,
            snapshot_a_time,
            800_000,
            1_200_000,
            160_000_000,
        )
        .await?;
        assert_match_total(&worker.pool, 0).await?;

        let event_one_time = snapshot_a_time + time::Duration::hours(1);
        let event_one = insert_cross_currency_product_with_event(
            &worker.pool,
            CrossCurrencyProductListingInput {
                title: &product_listing_query,
                event_time: event_one_time,
                price: Some((10_000, "GBP")),
                price_estimate_min: None,
                price_estimate_max: None,
                availability: "AVAILABLE",
                sale_observation_fx_rate_id: None,
            },
        )
        .await?;
        assert_product_source_price(&worker.pool, event_one.product_listing_id, 10_000, "GBP")
            .await?;
        wait_for_match(&worker.pool, event_one.event_id, all_filters.len() as i64).await?;
        assert_matches_for_event(
            &worker.pool,
            event_one.event_id,
            all_filters.map(SearchFilter::id),
        )
        .await?;
        assert_price_filter_valuations(
            &worker.pool,
            event_one.event_id,
            price_filters,
            "EVENT",
            snapshot_a,
        )
        .await?;
        assert_match_valuation(
            &worker.pool,
            event_one.event_id,
            no_price_filter.id(),
            None,
            None,
        )
        .await?;

        let snapshot_b = insert_fx_snapshot(
            &worker.pool,
            event_one_time + time::Duration::hours(1),
            800_000,
            1_200_000,
            160_000_000,
        )
        .await?;
        assert_ne!(snapshot_a, snapshot_b);
        assert_match_total_for_duration(&worker.pool, 4, NO_SIDE_EFFECT_OBSERVATION).await?;

        redeliver_product_event(
            &worker.server,
            event_one.product_listing_id,
            event_one.event_id,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
        )
        .await?;
        assert_match_count_for_duration(
            &worker.pool,
            event_one.event_id,
            all_filters.len() as i64,
            NO_SIDE_EFFECT_OBSERVATION,
        )
        .await?;
        assert_price_filter_valuations(
            &worker.pool,
            event_one.event_id,
            price_filters,
            "EVENT",
            snapshot_a,
        )
        .await?;

        let event_two = insert_cross_currency_product_with_event(
            &worker.pool,
            CrossCurrencyProductListingInput {
                title: &product_listing_query,
                event_time: event_one_time + time::Duration::hours(2),
                price: Some((10_000, "GBP")),
                price_estimate_min: None,
                price_estimate_max: None,
                availability: "AVAILABLE",
                sale_observation_fx_rate_id: None,
            },
        )
        .await?;
        wait_for_match(&worker.pool, event_two.event_id, all_filters.len() as i64).await?;
        assert_matches_for_event(
            &worker.pool,
            event_two.event_id,
            all_filters.map(SearchFilter::id),
        )
        .await?;
        assert_price_filter_valuations(
            &worker.pool,
            event_two.event_id,
            price_filters,
            "EVENT",
            snapshot_b,
        )
        .await?;
        assert_match_valuation(
            &worker.pool,
            event_two.event_id,
            no_price_filter.id(),
            None,
            None,
        )
        .await?;

        let snapshot_c = insert_fx_snapshot(
            &worker.pool,
            event_one_time + time::Duration::hours(3),
            1_000_000,
            900_000,
            120_000_000,
        )
        .await?;
        assert_ne!(snapshot_b, snapshot_c);
        assert_match_total_for_duration(&worker.pool, 8, NO_SIDE_EFFECT_OBSERVATION).await?;

        let sale_event = insert_cross_currency_product_with_event(
            &worker.pool,
            CrossCurrencyProductListingInput {
                title: &product_listing_query,
                event_time: event_one_time + time::Duration::hours(4),
                price: Some((10_000, "GBP")),
                price_estimate_min: None,
                price_estimate_max: None,
                availability: "SOLD_OUT",
                sale_observation_fx_rate_id: Some(snapshot_b),
            },
        )
        .await?;
        wait_for_match(&worker.pool, sale_event.event_id, all_filters.len() as i64).await?;
        assert_matches_for_event(
            &worker.pool,
            sale_event.event_id,
            all_filters.map(SearchFilter::id),
        )
        .await?;
        assert_price_filter_valuations(
            &worker.pool,
            sale_event.event_id,
            price_filters,
            "SALE_OBSERVATION",
            snapshot_b,
        )
        .await?;
        assert_match_valuation(
            &worker.pool,
            sale_event.event_id,
            no_price_filter.id(),
            None,
            None,
        )
        .await?;

        let no_price_event = insert_cross_currency_product_with_event(
            &worker.pool,
            CrossCurrencyProductListingInput {
                title: &product_listing_query,
                event_time: event_one_time + time::Duration::hours(4),
                price: None,
                price_estimate_min: Some((10_000, "GBP")),
                price_estimate_max: Some((10_000, "GBP")),
                availability: "AVAILABLE",
                sale_observation_fx_rate_id: None,
            },
        )
        .await?;
        wait_for_match(&worker.pool, no_price_event.event_id, 1).await?;
        assert_matches_for_event(
            &worker.pool,
            no_price_event.event_id,
            [no_price_filter.id()],
        )
        .await?;
        assert_match_valuation(
            &worker.pool,
            no_price_event.event_id,
            no_price_filter.id(),
            None,
            None,
        )
        .await?;
        assert_match_total(&worker.pool, 13).await?;
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn redelivery_and_deterministic_selection_flow() -> Result<(), Box<dyn std::error::Error>> {
    let worker = FullFlowWorker::start().await?;
    let result = async {
        let user_id = seed_user(&worker.pool, "ULTIMATE").await?;
        let product_listing_query = format!("Worker percolator product {user_id}");
        let first_filter = search_filter(
            user_id,
            UserSearchFilterName::from("First deterministic filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        let second_filter = search_filter(
            user_id,
            UserSearchFilterName::from("Second deterministic filter"),
            SearchFilterState::Active,
            &product_listing_query,
        )?;
        worker.project_filter(&first_filter).await?;
        worker.project_filter(&second_filter).await?;
        refresh_index("user_search_filters").await;

        let (product_listing_id, event_id) =
            create_product_with_domain_event(&worker.pool, &product_listing_query).await?;
        wait_for_match(&worker.pool, event_id, 2).await?;
        let notifications = wait_for_notifications(&worker.pool, user_id, 2).await?;
        for filter_id in [first_filter.id(), second_filter.id()] {
            let notification = notifications
                .iter()
                .find(|notification| {
                    notification.user_search_filter_id == uuid::Uuid::from(filter_id)
                })
                .ok_or_else(|| std::io::Error::other("filter notification is missing"))?;
            assert_search_filter_notification(
                notification,
                user_id,
                filter_id,
                product_listing_id,
                event_id,
            )?;
        }

        redeliver_product_event(
            &worker.server,
            product_listing_id,
            event_id,
            "PRODUCT_LISTING_DISCOVERED",
            "DOMAIN",
        )
        .await?;
        assert_match_count_for_duration(&worker.pool, event_id, 2, NO_SIDE_EFFECT_OBSERVATION)
            .await?;
        redeliver_search_filter_match(
            &worker.server,
            user_id,
            first_filter.id(),
            product_listing_id,
            event_id,
        )
        .await?;
        assert_no_more_than_notifications(&worker.pool, user_id, 2, NO_SIDE_EFFECT_OBSERVATION)
            .await
    }
    .await;

    worker.finish(result).await
}

struct FullFlowWorker {
    pool: sqlx::PgPool,
    index: OpenSearchSearchFilterIndex,
    server: ScopedWorkerServer,
    _unused_receivers: aura_historia_worker::cdc::WorkerQueueReceivers,
    percolator_consumer: JoinHandle<()>,
    notification_consumer: JoinHandle<()>,
}

impl FullFlowWorker {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        seed_current_fx_snapshot(&pool).await?;
        let index = OpenSearchSearchFilterIndex::new(get_opensearch_client().await.clone());
        let percolator_handler: Arc<dyn MatchProductListingEventUseCase> =
            Arc::new(MatchProductListingEventHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
                SqlxProductListingCurrentEventGuardFactory::new(),
                SqlxFxRateSnapshotRepositoryFactory,
                index.clone(),
                NonMatchingLargeLanguageModel,
                SqlxActiveSearchFilterMatchCandidateReaderFactory,
                SqlxSearchFilterMatchWriterFactory,
            ));
        let notification_handler: Arc<dyn GenerateSearchFilterMatchNotificationUseCase> =
            Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxSearchFilterMatchNotificationSourceReaderFactory,
                SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
                SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
                SqlxUserTierEntitlementsFactory::new(),
                SqlxProductListingContentAssessmentSnapshotReaderFactory::new(),
                NotificationCreationCoordinatorFactory::new(
                    SqlxNotificationRepositoryFactory::new(),
                    InitialExternalDeliveryPlanReaderFactory,
                    SqlxNotificationDeliveryIntentRepositoryFactory::new(),
                ),
            ));
        let (runtime, mut receivers) = WorkerRuntime::with_all_queues(QueueConfig::new(16))?;
        let percolator_receiver = receivers
            .take(WorkerQueue::SearchFilterPercolator)
            .ok_or_else(|| std::io::Error::other("search-filter percolator queue is missing"))?;
        let notification_receiver = receivers
            .take(WorkerQueue::SearchFilterMatchNotification)
            .ok_or_else(|| {
                std::io::Error::other("search-filter match notification queue is missing")
            })?;
        let percolator_consumer = tokio::spawn(consume_search_filter_percolator_queue(
            percolator_receiver,
            percolator_handler,
        ));
        let notification_consumer = tokio::spawn(consume_search_filter_match_notification_queue(
            notification_receiver,
            notification_handler,
        ));
        let server = ScopedWorkerServer::start(runtime).await?;

        Ok(Self {
            pool,
            index,
            server,
            _unused_receivers: receivers,
            percolator_consumer,
            notification_consumer,
        })
    }

    async fn project_filter(
        &self,
        filter: &SearchFilter,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let version = insert_filter(&self.pool, filter).await?;
        self.project_existing_filter(filter, version).await
    }

    async fn project_existing_filter(
        &self,
        filter: &SearchFilter,
        version: i64,
    ) -> Result<(), Box<dyn std::error::Error>> {
        ProjectSearchFilterChangeHandler::new(
            SqlxSearchFilterIndexReader::new(self.pool.clone()),
            self.index.clone(),
        )
        .execute(ProjectSearchFilterChangeCommand {
            search_filter_id: filter.id(),
            source_version: version,
            operation: SearchFilterProjectionOperation::Upsert,
        })
        .await?;
        Ok(())
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
        let Self {
            server,
            _unused_receivers,
            percolator_consumer,
            notification_consumer,
            ..
        } = self;
        let server_shutdown_result = server.shutdown().await;
        let (percolator_shutdown_result, notification_shutdown_result) =
            tokio::join!(percolator_consumer, notification_consumer);

        drop(_unused_receivers);
        server_shutdown_result?;
        percolator_shutdown_result?;
        notification_shutdown_result?;
        Ok(())
    }
}

struct PercolatorWorker {
    pool: sqlx::PgPool,
    index: OpenSearchSearchFilterIndex,
    server: ScopedWorkerServer,
    _unused_receivers: aura_historia_worker::cdc::WorkerQueueReceivers,
    consumer: JoinHandle<()>,
}

impl PercolatorWorker {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        seed_current_fx_snapshot(&pool).await?;
        let index = OpenSearchSearchFilterIndex::new(get_opensearch_client().await.clone());
        let handler: Arc<dyn MatchProductListingEventUseCase> =
            Arc::new(MatchProductListingEventHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
                SqlxProductListingCurrentEventGuardFactory::new(),
                SqlxFxRateSnapshotRepositoryFactory,
                index.clone(),
                NonMatchingLargeLanguageModel,
                SqlxActiveSearchFilterMatchCandidateReaderFactory,
                SqlxSearchFilterMatchWriterFactory,
            ));
        let (runtime, mut receivers) = WorkerRuntime::with_all_queues(QueueConfig::new(64))?;
        let receiver = receivers
            .take(WorkerQueue::SearchFilterPercolator)
            .ok_or_else(|| std::io::Error::other("search-filter percolator queue is missing"))?;
        let consumer = tokio::spawn(consume_search_filter_percolator_queue(receiver, handler));
        let server = ScopedWorkerServer::start(runtime).await?;

        Ok(Self {
            pool,
            index,
            server,
            _unused_receivers: receivers,
            consumer,
        })
    }

    async fn project_filter(
        &self,
        filter: &SearchFilter,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let version = insert_filter(&self.pool, filter).await?;
        ProjectSearchFilterChangeHandler::new(
            SqlxSearchFilterIndexReader::new(self.pool.clone()),
            self.index.clone(),
        )
        .execute(ProjectSearchFilterChangeCommand {
            search_filter_id: filter.id(),
            source_version: version,
            operation: SearchFilterProjectionOperation::Upsert,
        })
        .await?;
        Ok(())
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
        let Self {
            server,
            _unused_receivers,
            consumer,
            ..
        } = self;
        let server_shutdown_result = server.shutdown().await;
        drop(_unused_receivers);
        let consumer_shutdown_result = consumer.await;
        server_shutdown_result?;
        consumer_shutdown_result?;
        Ok(())
    }
}

struct ScopedWorkerServer {
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
}

impl ScopedWorkerServer {
    async fn start(runtime: WorkerRuntime) -> Result<Self, std::io::Error> {
        let listener = tokio::net::TcpListener::bind(get_sequin_worker_webhook_bind_addr()).await?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));

        Ok(Self {
            shutdown_tx,
            server,
        })
    }

    fn local_webhook_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/cdc/sequin",
            get_sequin_worker_webhook_bind_addr().port()
        )
    }

    async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        self.shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker server shutdown channel closed"))?;
        self.server.await??;
        Ok(())
    }
}

async fn seed_current_fx_snapshot(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    let fx_rate_id = FxRateId::new();
    sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, now(), $2, $3)",
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind("fxratesapi")
    .bind(fx_rate_id.to_string())
    .execute(pool)
    .await?;

    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF",
    ] {
        sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::from(fx_rate_id))
        .bind(currency)
        .bind(if currency == "EUR" {
            1_000_000_i64
        } else {
            1_250_000_i64
        })
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn seed_user(pool: &sqlx::PgPool, tier: &str) -> Result<UserId, sqlx::Error> {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, $3, 'USER')")
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("worker-percolator-{user_id}@example.test"))
        .bind(tier)
        .execute(pool)
        .await?;
    Ok(user_id)
}

async fn create_product_with_domain_event(
    pool: &sqlx::PgPool,
    title: &str,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    create_product_with_event(
        pool,
        title,
        "PRODUCT_LISTING_DISCOVERED",
        "DOMAIN",
        json!({
            "listingSourceId": "10000000-0000-0000-0000-000000000001",
            "sourceListingId": "fixture-source-id",
            "title": {"language": "en", "text": title},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }),
    )
    .await
}

async fn create_product_with_event(
    pool: &sqlx::PgPool,
    title: &str,
    event_type: &str,
    event_group: &str,
    payload: Value,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    create_product_with_event_and_lifecycle(pool, title, event_type, event_group, payload, "ACTIVE")
        .await
}

async fn create_product_with_event_and_lifecycle(
    pool: &sqlx::PgPool,
    title: &str,
    event_type: &str,
    event_group: &str,
    payload: Value,
    lifecycle: &str,
) -> Result<(ProductListingId, EventId), sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let product_uuid = uuid::Uuid::from(product_listing_id);
    let event_id = EventId::new();
    let listing_source_id = uuid::Uuid::new_v4();
    let product_slug_suffix = product_uuid.simple().to_string()[..6].to_owned();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), concat($3, ' operator')) RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, $3, party_id FROM operator")
        .bind(listing_source_id)
        .bind(format!("worker-percolator-source-{listing_source_id}"))
        .bind("Worker percolator source")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'en', 'Worker percolator description', 'en', CASE WHEN $7 = 'WITHDRAWN' THEN NULL ELSE 'AVAILABLE' END, $7, 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(format!("worker-percolator-product-{product_slug_suffix}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(listing_source_id)
        .bind(product_uuid.to_string())
        .bind(title)
        .bind(lifecycle)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(product_uuid)
        .bind(event_type)
        .bind(event_group)
        .bind(payload)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((product_listing_id, event_id))
}

struct CrossCurrencyProductListingEvent {
    product_listing_id: ProductListingId,
    event_id: EventId,
}

struct CrossCurrencyProductListingInput<'a> {
    title: &'a str,
    event_time: time::OffsetDateTime,
    price: Option<(i64, &'a str)>,
    price_estimate_min: Option<(i64, &'a str)>,
    price_estimate_max: Option<(i64, &'a str)>,
    availability: &'a str,
    sale_observation_fx_rate_id: Option<FxRateId>,
}

async fn insert_cross_currency_product_with_event(
    pool: &sqlx::PgPool,
    input: CrossCurrencyProductListingInput<'_>,
) -> Result<CrossCurrencyProductListingEvent, sqlx::Error> {
    let CrossCurrencyProductListingInput {
        title,
        event_time,
        price,
        price_estimate_min,
        price_estimate_max,
        availability,
        sale_observation_fx_rate_id,
    } = input;
    let product_listing_id = ProductListingId::new();
    let product_uuid = uuid::Uuid::from(product_listing_id);
    let event_id = EventId::new();
    let discovery_event_id = if sale_observation_fx_rate_id.is_some() {
        EventId::new()
    } else {
        event_id
    };
    let listing_source_id = uuid::Uuid::new_v4();
    let product_slug_suffix = product_uuid.simple().to_string()[..6].to_owned();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, 'Cross currency worker source', party_id FROM operator")
        .bind(listing_source_id)
        .bind(format!("cross-currency-worker-source-{listing_source_id}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, price_amount, price_currency, price_estimate_min_amount, price_estimate_min_currency, price_estimate_max_amount, price_estimate_max_currency, sale_observation_fx_rate_id, sale_observed_at, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'en', 'Cross currency worker description', 'en', $7, $8, $9, $10, $11, $12, $13, CASE WHEN $13 IS NULL THEN NULL ELSE $14 END, $15, 'ACTIVE', 'https://example.test/cross-currency-product', '[]')",
    )
    .bind(product_uuid)
    .bind(format!("cross-currency-worker-product-{product_slug_suffix}"))
    .bind(uuid::Uuid::from(event_id))
    .bind(listing_source_id)
    .bind(product_uuid.to_string())
    .bind(title)
    .bind(price.map(|(amount, _)| amount))
    .bind(price.map(|(_, currency)| currency))
    .bind(price_estimate_min.map(|(amount, _)| amount))
    .bind(price_estimate_min.map(|(_, currency)| currency))
    .bind(price_estimate_max.map(|(amount, _)| amount))
    .bind(price_estimate_max.map(|(_, currency)| currency))
    .bind(sale_observation_fx_rate_id.map(uuid::Uuid::from))
    .bind(event_time)
    .bind(availability)

        .execute(&mut *tx)
    .await?;
    let payload = serde_json::json!({
        "listingSourceId": listing_source_id.to_string(),
        "sourceListingId": product_uuid.to_string(),
        "title": {"language": "en", "text": title},
        "description": {"language": "en", "text": "Cross currency worker description"},
        "pricing": {
            "price": price.map(|(amount, currency)| serde_json::json!({"amount": amount, "currency": currency})),
            "priceEstimateMin": price_estimate_min.map(|(amount, currency)| serde_json::json!({"amount": amount, "currency": currency})),
            "priceEstimateMax": price_estimate_max.map(|(amount, currency)| serde_json::json!({"amount": amount, "currency": currency}))
        },
        "availability": availability,
        "url": "https://example.test/cross-currency-product",
        "imageCount": 0,
        "auction": {"start": null, "end": null}
    });
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, $4)")
        .bind(uuid::Uuid::from(discovery_event_id))
        .bind(product_uuid)
        .bind(payload)
        .bind(event_time)
        .execute(&mut *tx)
        .await?;
    if let Some(fx_rate_id) = sale_observation_fx_rate_id {
        let observed_at = event_time
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| {
                sqlx::Error::Protocol("invalid fixture observation timestamp".to_owned())
            })?;
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, $4)")
            .bind(uuid::Uuid::from(event_id))
            .bind(product_uuid)
            .bind(serde_json::json!({
                "saleObservation": {
                    "transition": "OBSERVED",
                    "observation": {
                        "observedAt": observed_at,
                        "fxRateId": fx_rate_id.to_string()
                    }
                }
            }))
            .bind(event_time)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE product_listings SET content_source_event_id = $1, embedding_source_event_id = $1, version = version + 1, projection_version = projection_version + 1 WHERE product_listing_id = $2")
            .bind(uuid::Uuid::from(discovery_event_id))
            .bind(product_uuid)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(CrossCurrencyProductListingEvent {
        product_listing_id,
        event_id,
    })
}

async fn insert_fx_snapshot(
    pool: &sqlx::PgPool,
    captured_at: time::OffsetDateTime,
    gbp_units_per_eur: i64,
    usd_units_per_eur: i64,
    jpy_units_per_eur: i64,
) -> Result<FxRateId, sqlx::Error> {
    let fx_rate_id = FxRateId::new();
    sqlx::query(
        "INSERT INTO fx_rates (fx_rate_id, captured_at, source, source_event_id) VALUES ($1, $2, 'fxratesapi', $3)",
    )
    .bind(uuid::Uuid::from(fx_rate_id))
    .bind(captured_at)
    .bind(fx_rate_id.to_string())
    .execute(pool)
    .await?;
    for currency in [
        "EUR", "GBP", "USD", "AUD", "CAD", "NZD", "CNY", "BRL", "PLN", "TRY", "JPY", "CZK", "RUB",
        "AED", "SAR", "HKD", "SGD", "CHF",
    ] {
        let units_per_eur = match currency {
            "EUR" => 1_000_000,
            "GBP" => gbp_units_per_eur,
            "USD" => usd_units_per_eur,
            "JPY" => jpy_units_per_eur,
            _ => 1_250_000,
        };
        sqlx::query(
            "INSERT INTO fx_rate_quotes (fx_rate_id, currency, units_per_eur) VALUES ($1, $2, $3)",
        )
        .bind(uuid::Uuid::from(fx_rate_id))
        .bind(currency)
        .bind(units_per_eur)
        .execute(pool)
        .await?;
    }
    Ok(fx_rate_id)
}

fn search_filter(
    user_id: UserId,
    name: UserSearchFilterName,
    state: SearchFilterState,
    product_listing_query: &str,
) -> Result<SearchFilter, Box<dyn std::error::Error>> {
    Ok(SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name,
        notifications: true,
        state,
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query(product_listing_query.try_into()?),
        embedding: None,
    }))
}

fn price_search_filter(
    user_id: UserId,
    name: UserSearchFilterName,
    product_listing_query: &str,
    currency: Currency,
    minimum: u64,
    maximum: u64,
) -> Result<SearchFilter, Box<dyn std::error::Error>> {
    Ok(SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name,
        notifications: true,
        state: SearchFilterState::Active,
        search: ProductListingSearch::new(Language::En, currency)
            .with_product_listing_query(product_listing_query.try_into()?)
            .with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(minimum)),
                max: Some(MonetaryAmount::from(maximum)),
            }),
        embedding: None,
    }))
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

async fn withdraw_product_listing_and_insert_event(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_CHANGED', 'DOMAIN', 1, $3, now())",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(json!({
        "availability": {"previous": "AVAILABLE", "current": null},
        "lifecycle": {"transition": "WITHDRAWN", "previousAvailability": "AVAILABLE"}
    }))
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE product_listings SET current_event_id = $1, lifecycle = 'WITHDRAWN', availability = NULL, version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::from(product_listing_id))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn update_product_and_insert_event(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    title: &str,
) -> Result<EventId, sqlx::Error> {
    update_product_and_insert_event_with_group(
        pool,
        product_listing_id,
        title,
        "PRODUCT_LISTING_CHANGED",
        "DOMAIN",
    )
    .await
}

async fn update_product_and_insert_event_with_group(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    title: &str,
    event_type: &str,
    event_group: &str,
) -> Result<EventId, sqlx::Error> {
    let event_id = EventId::new();
    let mut tx = pool.begin().await?;
    let payload = match (event_type, event_group) {
        ("PRODUCT_LISTING_CHANGED", "DOMAIN") => serde_json::json!({
            "images": {"previousCount": 0, "currentCount": 0}
        }),
        ("ENRICHMENT_EMBEDDED", "ENRICHMENT") => serde_json::json!({
            "sourceEventId": event_id.to_string()
        }),
        _ => serde_json::json!({}),
    };
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, $3, $4, 1, $5, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(uuid::Uuid::from(product_listing_id))
        .bind(event_type)
        .bind(event_group)
        .bind(payload)
        .execute(&mut *tx)
        .await?;
    let update_query = if event_group == "DOMAIN" {
        "UPDATE product_listings SET current_event_id = $1, title_text = $2, version = version + 1, projection_version = projection_version + 1, updated = now() WHERE product_listing_id = $3"
    } else {
        "UPDATE product_listings SET current_event_id = $1, title_text = $2, updated = now() WHERE product_listing_id = $3"
    };
    sqlx::query(update_query)
        .bind(uuid::Uuid::from(event_id))
        .bind(title)
        .bind(uuid::Uuid::from(product_listing_id))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(event_id)
}

async fn create_product_with_event_then_rollback(
    pool: &sqlx::PgPool,
    title: &str,
) -> Result<EventId, sqlx::Error> {
    let product_listing_id = ProductListingId::new();
    let product_uuid = uuid::Uuid::from(product_listing_id);
    let event_id = EventId::new();
    let listing_source_id = uuid::Uuid::new_v4();
    let product_slug_suffix = product_uuid.simple().to_string()[..6].to_owned();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), concat($3, ' operator')) RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $1, $2, $3, party_id FROM operator")
        .bind(listing_source_id)
        .bind(format!("worker-percolator-source-{listing_source_id}"))
        .bind("Worker percolator source")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, $6, 'en', 'Worker percolator description', 'en', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_uuid)
        .bind(format!("worker-percolator-product-{product_slug_suffix}"))
        .bind(uuid::Uuid::from(event_id))
        .bind(listing_source_id)
        .bind(product_uuid.to_string())
        .bind(title)

        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(uuid::Uuid::from(event_id))
        .bind(product_uuid)
        .bind(serde_json::json!({
            "listingSourceId": listing_source_id.to_string(),
            "sourceListingId": product_uuid.to_string(),
            "title": {"language": "en", "text": title},
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }))
        .execute(&mut *tx)
        .await?;
    drop(tx);
    Ok(event_id)
}

async fn insert_historical_search_filter_match(
    pool: &sqlx::PgPool,
    user_id: UserId,
    search_filter_id: UserSearchFilterId,
    product_listing_id: ProductListingId,
    origin_event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query(
        "INSERT INTO search_filter_matches (user_id, user_search_filter_id, product_listing_id, origin_event_id, user_search_filter_name, created, updated) VALUES ($1, $2, $3, $4, $5, GREATEST(date_trunc('month', now()), now() - INTERVAL '1 minute'), GREATEST(date_trunc('month', now()), now() - INTERVAL '1 minute'))",
    )
    .bind(uuid::Uuid::from(user_id))
    .bind(uuid::Uuid::parse_str(&search_filter_id.to_string())?)
    .bind(uuid::Uuid::from(product_listing_id))
    .bind(uuid::Uuid::from(origin_event_id))
    .bind("Free tier quota filter")
    .execute(pool)
    .await?;
    Ok(())
}

async fn product_event_type(pool: &sqlx::PgPool, event_id: EventId) -> Result<String, sqlx::Error> {
    sqlx::query_scalar("SELECT event_type FROM product_listing_events WHERE event_id = $1")
        .bind(uuid::Uuid::from(event_id))
        .fetch_one(pool)
        .await
}

async fn assert_product_listing_is_current_and_withdrawn(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    event_id: EventId,
) -> Result<(), sqlx::Error> {
    let (current_event_id, lifecycle, availability): (uuid::Uuid, String, Option<String>) =
        sqlx::query_as(
            "SELECT current_event_id, lifecycle, availability FROM product_listings WHERE product_listing_id = $1",
        )
        .bind(uuid::Uuid::from(product_listing_id))
        .fetch_one(pool)
        .await?;

    assert_eq!(uuid::Uuid::from(event_id), current_event_id);
    assert_eq!("WITHDRAWN", lifecycle);
    assert_eq!(None, availability);
    Ok(())
}

async fn assert_product_source_price(
    pool: &sqlx::PgPool,
    product_listing_id: ProductListingId,
    expected_amount: i64,
    expected_currency: &str,
) -> Result<(), sqlx::Error> {
    let (amount, currency): (i64, String) = sqlx::query_as(
        "SELECT price_amount, price_currency FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(product_listing_id))
    .fetch_one(pool)
    .await?;
    assert_eq!(expected_amount, amount);
    assert_eq!(expected_currency, currency);
    Ok(())
}

async fn wait_for_match(
    pool: &sqlx::PgPool,
    event_id: EventId,
    expected: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        if match_count(pool, event_id).await? == expected {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let actual = match_count(pool, event_id).await?;
    Err(std::io::Error::other(format!(
        "product event {event_id} created {actual} search-filter matches; expected {expected}"
    ))
    .into())
}

async fn match_count(pool: &sqlx::PgPool, event_id: EventId) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM search_filter_matches WHERE origin_event_id = $1")
        .bind(uuid::Uuid::from(event_id))
        .fetch_one(pool)
        .await
}

async fn matches_for_event(
    pool: &sqlx::PgPool,
    event_id: EventId,
) -> Result<Vec<UserSearchFilterId>, Box<dyn std::error::Error>> {
    let ids = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT user_search_filter_id FROM search_filter_matches WHERE origin_event_id = $1 ORDER BY user_search_filter_id",
    )
    .bind(uuid::Uuid::from(event_id))
    .fetch_all(pool)
    .await?;
    Ok(ids.into_iter().map(UserSearchFilterId::from).collect())
}

async fn assert_matches_for_event(
    pool: &sqlx::PgPool,
    event_id: EventId,
    expected: impl IntoIterator<Item = UserSearchFilterId>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut expected = expected.into_iter().collect::<Vec<_>>();
    expected.sort_by_key(ToString::to_string);
    assert_eq!(expected, matches_for_event(pool, event_id).await?);
    Ok(())
}

async fn assert_match_valuation(
    pool: &sqlx::PgPool,
    event_id: EventId,
    filter_id: UserSearchFilterId,
    expected_basis: Option<&str>,
    expected_fx_rate_id: Option<FxRateId>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (basis, fx_rate_id): (Option<String>, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT price_valuation_basis, price_fx_rate_id FROM search_filter_matches WHERE origin_event_id = $1 AND user_search_filter_id = $2",
    )
    .bind(uuid::Uuid::from(event_id))
    .bind(uuid::Uuid::parse_str(&filter_id.to_string())?)
    .fetch_one(pool)
    .await?;
    assert_eq!(expected_basis, basis.as_deref());
    assert_eq!(expected_fx_rate_id.map(uuid::Uuid::from), fx_rate_id);
    Ok(())
}

async fn assert_price_filter_valuations(
    pool: &sqlx::PgPool,
    event_id: EventId,
    filters: [&SearchFilter; 3],
    expected_basis: &str,
    expected_fx_rate_id: FxRateId,
) -> Result<(), Box<dyn std::error::Error>> {
    for filter in filters {
        assert_match_valuation(
            pool,
            event_id,
            filter.id(),
            Some(expected_basis),
            Some(expected_fx_rate_id),
        )
        .await?;
    }
    Ok(())
}

async fn assert_match_for_event(
    pool: &sqlx::PgPool,
    event_id: EventId,
    user_id: UserId,
    search_filter_id: UserSearchFilterId,
    product_listing_id: ProductListingId,
) -> Result<(), Box<dyn std::error::Error>> {
    let (matched_user_id, matched_search_filter_id, matched_product_listing_id): (
        uuid::Uuid,
        uuid::Uuid,
        uuid::Uuid,
    ) = sqlx::query_as(
        "SELECT user_id, user_search_filter_id, product_listing_id FROM search_filter_matches WHERE origin_event_id = $1",
    )
    .bind(uuid::Uuid::from(event_id))
    .fetch_one(pool)
    .await?;

    assert_eq!(uuid::Uuid::from(user_id), matched_user_id);
    assert_eq!(
        uuid::Uuid::parse_str(&search_filter_id.to_string())?,
        matched_search_filter_id
    );
    assert_eq!(
        uuid::Uuid::from(product_listing_id),
        matched_product_listing_id
    );
    Ok(())
}

async fn assert_match_total(
    pool: &sqlx::PgPool,
    expected: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let actual: i64 = sqlx::query_scalar("SELECT count(*) FROM search_filter_matches")
        .fetch_one(pool)
        .await?;
    assert_eq!(expected, actual);
    Ok(())
}

async fn assert_match_total_for_duration(
    pool: &sqlx::PgPool,
    expected: i64,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + duration;
    loop {
        assert_match_total(pool, expected).await?;
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn assert_match_count_for_duration(
    pool: &sqlx::PgPool,
    event_id: EventId,
    expected: i64,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + duration;
    loop {
        if match_count(pool, event_id).await? != expected {
            return Err(std::io::Error::other(format!(
                "product event {event_id} did not remain at {expected} search-filter matches"
            ))
            .into());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn assert_no_matches_for(
    pool: &sqlx::PgPool,
    event_id: EventId,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_match_count_for_duration(pool, event_id, 0, duration).await
}

async fn assert_event_is_not_persisted(
    pool: &sqlx::PgPool,
    event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM product_listing_events WHERE event_id = $1)",
    )
    .bind(uuid::Uuid::from(event_id))
    .fetch_one(pool)
    .await?;
    assert!(
        !exists,
        "rolled-back product event {event_id} was persisted"
    );
    Ok(())
}

#[derive(sqlx::FromRow)]
struct SearchFilterNotificationRow {
    user_id: uuid::Uuid,
    origin_event_id: uuid::Uuid,
    product_listing_id: uuid::Uuid,
    user_search_filter_id: uuid::Uuid,
    kind: String,
}

async fn notifications_for_user(
    pool: &sqlx::PgPool,
    user_id: UserId,
) -> Result<Vec<SearchFilterNotificationRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT user_id, origin_event_id, product_listing_id, user_search_filter_id, kind \
         FROM notifications WHERE user_id = $1 ORDER BY created, notification_id",
    )
    .bind(uuid::Uuid::from(user_id))
    .fetch_all(pool)
    .await
}

async fn wait_for_notifications(
    pool: &sqlx::PgPool,
    user_id: UserId,
    expected: usize,
) -> Result<Vec<SearchFilterNotificationRow>, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        let notifications = notifications_for_user(pool, user_id).await?;
        if notifications.len() == expected {
            return Ok(notifications);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "user {user_id} did not receive {expected} notifications"
    ))
    .into())
}

async fn assert_no_more_than_notifications(
    pool: &sqlx::PgPool,
    user_id: UserId,
    maximum: usize,
    duration: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = Instant::now() + duration;
    loop {
        let notifications = notifications_for_user(pool, user_id).await?;
        if notifications.len() > maximum {
            return Err(std::io::Error::other(format!(
                "user {user_id} received {} notifications; expected at most {maximum}",
                notifications.len()
            ))
            .into());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn assert_search_filter_notification(
    notification: &SearchFilterNotificationRow,
    user_id: UserId,
    search_filter_id: UserSearchFilterId,
    product_listing_id: ProductListingId,
    origin_event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(uuid::Uuid::from(user_id), notification.user_id);
    assert_eq!(
        uuid::Uuid::from(origin_event_id),
        notification.origin_event_id
    );
    assert_eq!(
        uuid::Uuid::from(product_listing_id),
        notification.product_listing_id
    );
    assert_eq!(
        uuid::Uuid::parse_str(&search_filter_id.to_string())?,
        notification.user_search_filter_id
    );
    assert_eq!("SEARCH_FILTER_MATCH", notification.kind);
    Ok(())
}

async fn assert_price_filter_document(
    filter: &SearchFilter,
    currency: Currency,
    minimum: u64,
    maximum: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let response: Value = get_opensearch_client()
        .await
        .get(GetParts::IndexId(
            "user_search_filters",
            &filter.id().to_string(),
        ))
        .send()
        .await?
        .error_for_status_code()?
        .json()
        .await?;
    let document = response
        .get("_source")
        .ok_or_else(|| std::io::Error::other("saved-filter OpenSearch response has no _source"))?;
    let price_field = match currency {
        Currency::Eur => "priceByCurrency.eur",
        Currency::Usd => "priceByCurrency.usd",
        Currency::Jpy => "priceByCurrency.jpy",
        _ => return Err(std::io::Error::other("unsupported currency assertion").into()),
    };
    let price_query = document
        .pointer("/query/bool/filter")
        .and_then(Value::as_array)
        .and_then(|filters| {
            filters
                .iter()
                .find_map(|filter| filter.get("range").and_then(|range| range.get(price_field)))
        })
        .ok_or_else(|| std::io::Error::other("saved-filter document has no price range query"))?;

    assert_eq!(
        Some(minimum),
        document
            .pointer("/search/price/min")
            .and_then(Value::as_u64)
    );
    assert_eq!(
        Some(maximum),
        document
            .pointer("/search/price/max")
            .and_then(Value::as_u64)
    );
    assert_eq!(
        Some(minimum),
        price_query.get("gte").and_then(Value::as_u64)
    );
    assert_eq!(
        Some(maximum),
        price_query.get("lte").and_then(Value::as_u64)
    );
    let serialized = document.to_string();
    assert!(!serialized.contains("fxRate"));
    assert!(!serialized.contains("generation"));
    assert!(!serialized.contains("Estimate"));
    assert!(!serialized.contains("estimate"));
    Ok(())
}

async fn redeliver_product_event(
    server: &ScopedWorkerServer,
    product_listing_id: ProductListingId,
    event_id: EventId,
    event_type: &str,
    event_group: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let payload = match (event_type, event_group) {
        ("PRODUCT_LISTING_DISCOVERED", "DOMAIN") => json!({
            "listingSourceId": "10000000-0000-0000-0000-000000000001",
            "sourceListingId": "fixture-source-id",
            "title": null,
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": "AVAILABLE",
            "url": "https://example.test/product",
            "imageCount": 0,
            "auction": {"start": null, "end": null}
        }),
        ("PRODUCT_LISTING_CHANGED", "DOMAIN") => json!({
            "availability": {"previous": null, "current": "AVAILABLE"}
        }),
        ("ENRICHMENT_EMBEDDED", "ENRICHMENT") => json!({
            "sourceEventId": event_id.to_string()
        }),
        _ => json!({}),
    };
    post_sequin_change(
        server.local_webhook_url(),
        json!({
            "record": {
                "event_id": event_id.to_string(),
                "product_listing_id": product_listing_id.to_string(),
                "event_type": event_type,
                "event_group": event_group,
                "event_type_schema_version": 1,
                "payload": payload
            },
            "action": "insert",
            "metadata": {"table_schema": "public", "table_name": "product_listing_events"}
        }),
    )
    .await
}

async fn redeliver_search_filter_match(
    server: &ScopedWorkerServer,
    user_id: UserId,
    search_filter_id: UserSearchFilterId,
    product_listing_id: ProductListingId,
    origin_event_id: EventId,
) -> Result<(), Box<dyn std::error::Error>> {
    post_sequin_change(
        server.local_webhook_url(),
        json!({
            "record": {
                "user_id": user_id.to_string(),
                "user_search_filter_id": search_filter_id.to_string(),
                "product_listing_id": product_listing_id.to_string(),
                "origin_event_id": origin_event_id.to_string()
            },
            "action": "insert",
            "metadata": {"table_schema": "public", "table_name": "search_filter_matches"}
        }),
    )
    .await
}

async fn post_sequin_change(
    webhook_url: String,
    change: serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    let response = reqwest::Client::new()
        .post(webhook_url)
        .json(&change)
        .send()
        .await?;
    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    Ok(())
}

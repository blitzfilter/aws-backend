use super::{DEFAULT_INDEX, OpenSearchSearchFilterIndex, paused_write::PausedWrite};
use crate::document::SearchFilterDocument;
use domain_primitives::event_id::EventId;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use localization::Language;
use money::Currency;
use opensearch::{GetParts, http::StatusCode, indices::IndicesPutSettingsParts};
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
    product_listing_search::ProductListingSearch, product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
};
use product_listing_service::ports::{
    ListingSourceSummary, ProductListingPercolationInput, ProductListingSearchFilterMatchSource,
    ProductListingSearchFilterMatchSourceEventKind,
};
use search_filter_core::{
    search_filter_state::SearchFilterState, user_search_filter_id::UserSearchFilterId,
    user_search_filter_name::UserSearchFilterName,
};
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterIndexQuery, SearchFilterProjection,
    SearchFilterProjectionWriteOutcome, SearchFilterView,
};
use serde_json::json;
use std::time::Duration;
use test_api::{
    IntegrationTestService, OpenSearch as TestOpenSearch, aura_integration_test,
    get_opensearch_client, refresh_index,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [TestOpenSearch()])]
async fn should_fence_in_flight_filter_writes_after_delete_gc_and_newer_projection() {
    let result = deletion_race().await;
    assert!(
        result.is_ok(),
        "real OpenSearch filter deletion race: {result:?}"
    );
}

async fn deletion_race() -> TestResult {
    let client = get_opensearch_client().await.clone();
    client
        .indices()
        .put_settings(IndicesPutSettingsParts::Index(&[DEFAULT_INDEX]))
        .body(json!({"index.gc_deletes": "60s"}))
        .send()
        .await?
        .error_for_status_code()?;
    let index = OpenSearchSearchFilterIndex::new(client.clone());
    let mut projection = projection();
    let id = projection.view.search_filter_id;
    assert_eq!(
        SearchFilterProjectionWriteOutcome::Applied,
        index.upsert(&projection).await?
    );
    assert_visible(&index, 1).await?;

    projection.source_version = 2;
    let mut paused = PausedWrite::new(client.clone()).await?;
    let delayed_index = OpenSearchSearchFilterIndex::new(paused.client.clone());
    let stale_projection = projection.clone();
    let delayed = tokio::spawn(async move { delayed_index.upsert(&stale_projection).await });
    paused.wait_until_received().await?;
    let (first, duplicate) = tokio::join!(index.delete(id, 3), index.delete(id, 3));
    let outcomes = [first?, duplicate?];
    assert!(outcomes.contains(&SearchFilterProjectionWriteOutcome::Applied));
    assert!(outcomes.contains(&SearchFilterProjectionWriteOutcome::Stale));

    let mut unseen = projection.clone();
    unseen.view.search_filter_id = UserSearchFilterId::new();
    let unseen_id = unseen.view.search_filter_id;
    let mut paused_unseen = PausedWrite::new(client.clone()).await?;
    let unseen_index = OpenSearchSearchFilterIndex::new(paused_unseen.client.clone());
    let delayed_unseen = tokio::spawn(async move { unseen_index.upsert(&unseen).await });
    paused_unseen.wait_until_received().await?;
    assert_eq!(
        SearchFilterProjectionWriteOutcome::Applied,
        index.delete(unseen_id, 3).await?
    );

    tokio::time::sleep(Duration::from_secs(65)).await;
    assert_eq!(StatusCode::CONFLICT, paused.resume().await?);
    assert_eq!(SearchFilterProjectionWriteOutcome::Stale, delayed.await??);
    assert_eq!(StatusCode::CONFLICT, paused_unseen.resume().await?);
    assert_eq!(
        SearchFilterProjectionWriteOutcome::Stale,
        delayed_unseen.await??
    );
    for deleted_id in [id, unseen_id] {
        let stored = stored(deleted_id).await?;
        assert_eq!(json!(deleted_id.to_string()), stored["_id"]);
        assert_eq!(json!(3), stored["_version"]);
        assert_eq!(
            json!({"userSearchFilterId": deleted_id, "sourceVersion": 3, "projectionDeleted": true}),
            stored["_source"]
        );
        assert_eq!(
            SearchFilterProjectionWriteOutcome::Stale,
            index.delete(deleted_id, 3).await?
        );
    }
    assert_visible(&index, 0).await?;

    let mut paused_delete = PausedWrite::new(client).await?;
    let delayed_index = OpenSearchSearchFilterIndex::new(paused_delete.client.clone());
    let delayed_delete = tokio::spawn(async move { delayed_index.delete(id, 4).await });
    paused_delete.wait_until_received().await?;
    projection.source_version = 5;
    assert_eq!(
        SearchFilterProjectionWriteOutcome::Applied,
        index.upsert(&projection).await?
    );
    assert_eq!(StatusCode::CONFLICT, paused_delete.resume().await?);
    assert_eq!(
        SearchFilterProjectionWriteOutcome::Stale,
        delayed_delete.await??
    );
    assert_eq!(
        SearchFilterProjectionWriteOutcome::Stale,
        index.upsert(&projection).await?
    );
    let restored = stored(id).await?;
    assert_eq!(json!(id.to_string()), restored["_id"]);
    assert_eq!(json!(5), restored["_version"]);
    assert_eq!(
        serde_json::to_value(SearchFilterDocument::try_from(&projection)?)?,
        restored["_source"]
    );
    assert_visible(&index, 1).await?;
    Ok(())
}

async fn stored(id: UserSearchFilterId) -> TestResult<serde_json::Value> {
    Ok(get_opensearch_client()
        .await
        .get(GetParts::IndexId(DEFAULT_INDEX, &id.to_string()))
        .send()
        .await?
        .error_for_status_code()?
        .json()
        .await?)
}

async fn assert_visible(index: &OpenSearchSearchFilterIndex, expected: usize) -> TestResult {
    refresh_index(DEFAULT_INDEX).await;
    let result = index.query(&SearchFilterIndexQuery::default()).await?;
    assert_eq!(expected, result.items.len());
    assert_eq!(Some(expected as u64), result.total);
    assert_eq!(expected, index.percolate(&input()?).await?.len());
    Ok(())
}

fn projection() -> SearchFilterProjection {
    SearchFilterProjection {
        source_version: 1,
        view: SearchFilterView {
            search_filter_id: UserSearchFilterId::new(),
            user_id: UserId::new(),
            name: UserSearchFilterName::from("Fenced filter"),
            notifications: true,
            state: SearchFilterState::Active,
            search: ProductListingSearch::new(Language::En, Currency::Eur),
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        },
    }
}

fn input() -> TestResult<ProductListingPercolationInput> {
    let event_id = EventId::new();
    Ok(ProductListingPercolationInput {
        valuation: None,
        source: ProductListingSearchFilterMatchSource {
            event_id,
            current_event_id: event_id,
            projection_version: 1,
            event_kind: ProductListingSearchFilterMatchSourceEventKind::Domain,
            origin_event_time: OffsetDateTime::UNIX_EPOCH,
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("fenced-listing-a1b2c3")?,
            source: ListingSourceSummary {
                listing_source_id: ListingSourceId::new(),
                name: ListingSourceName::try_from("Source")?,
                slug_id: ListingSourceSlugId::raw("source")?,
            },
            source_listing_id: SourceListingId::try_from("sku-1")?,
            product_title: None,
            product_description: None,
            titles: Default::default(),
            descriptions: Default::default(),
            pricing: Default::default(),
            sale_observation: None,
            availability: None,
            lifecycle: ListingLifecycle::Active,
            url: "https://example.test/listing".parse()?,
            view_url: "https://example.test/listing".parse()?,
            image: None,
            images: Default::default(),
            embedding: None,
            auction: Default::default(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        },
    })
}

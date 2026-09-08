use super::{DEFAULT_INDEX, OpenSearchProductListingSearchProjection, paused_write::PausedWrite};
use crate::{
    OpenSearchProductListingSearchReader, OpenSearchProductListingSimilarProductListingsReader,
    product_listing_percolation_document::product_listing_document,
};
use domain_primitives::event_id::EventId;
use fxrate_core::{FX_RATE_SCALE, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use localization::{Language, Localized};
use money::Currency;
use opensearch::{
    DeleteParts, GetParts, IndexParts, http::StatusCode, indices::IndicesPutSettingsParts,
    params::VersionType,
};
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
    product_listing_search::ProductListingSearch, product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId, title::Title,
};
use product_listing_service::ports::{
    CompiledProductListingSearch, ListingSourceSummary, ProductListingPriceFilterPlan,
    ProductListingSearchFilterMatchSource, ProductListingSearchFilterMatchSourceEventKind,
    ProductListingSearchProjection, ProductListingSearchProjectionWriteOutcome,
    ProductListingSearchReadRequest, ProductListingSearchReader,
    ProductListingSimilarProductListingsReader, ProductListingSimilarProductListingsRequest,
};
use serde_json::{Value, json};
use std::time::Duration;
use strum::IntoEnumIterator;
use test_api::{
    IntegrationTestService, OpenSearch as TestOpenSearch, aura_integration_test,
    get_opensearch_client, refresh_index,
};
use time::OffsetDateTime;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [TestOpenSearch()])]
async fn should_fence_in_flight_writes_after_withdrawal_gc_and_restore() {
    let result = withdrawal_race().await;
    assert!(
        result.is_ok(),
        "real OpenSearch withdrawal race: {result:?}"
    );
}

async fn withdrawal_race() -> TestResult {
    let client = get_opensearch_client().await.clone();
    client
        .indices()
        .put_settings(IndicesPutSettingsParts::Index(&[DEFAULT_INDEX]))
        .body(json!({"index.gc_deletes": "60s"}))
        .send()
        .await?
        .error_for_status_code()?;
    let writer = OpenSearchProductListingSearchProjection::new(client.clone());
    let mut source = source()?;
    assert_eq!(
        ProductListingSearchProjectionWriteOutcome::Applied,
        writer.upsert(&source, None).await?
    );
    assert_visible(&source, 1).await?;

    source.projection_version = 2;
    source.event_id = EventId::new();
    source.current_event_id = source.event_id;
    let mut paused = PausedWrite::new(client.clone()).await?;
    let delayed_writer = OpenSearchProductListingSearchProjection::new(paused.client.clone());
    let stale_source = source.clone();
    let delayed = tokio::spawn(async move { delayed_writer.upsert(&stale_source, None).await });
    paused.wait_until_received().await?;

    // Deletion must also fence a write when no prior projection exists.
    let mut unseen = source.clone();
    unseen.product_listing_id = ProductListingId::new();
    let unseen_id = unseen.product_listing_id;
    let mut paused_unseen = PausedWrite::new(client.clone()).await?;
    let unseen_writer = OpenSearchProductListingSearchProjection::new(paused_unseen.client.clone());
    let delayed_unseen = tokio::spawn(async move { unseen_writer.upsert(&unseen, None).await });
    paused_unseen.wait_until_received().await?;
    let (first, duplicate) = tokio::join!(
        writer.delete(source.product_listing_id, 3),
        writer.delete(source.product_listing_id, 3)
    );
    let outcomes = [first?, duplicate?];
    assert!(outcomes.contains(&ProductListingSearchProjectionWriteOutcome::Applied));
    assert!(outcomes.contains(&ProductListingSearchProjectionWriteOutcome::Stale));
    assert_eq!(
        ProductListingSearchProjectionWriteOutcome::Applied,
        writer.delete(unseen_id, 3).await?
    );

    // Control proves real DELETE version memory expired, rather than merely sleeping.
    // It carries the marker so the protocol probe cannot pollute supported searches.
    let control_id = ProductListingId::new().to_string();
    client
        .index(IndexParts::IndexId(DEFAULT_INDEX, &control_id))
        .version(1)
        .version_type(VersionType::External)
        .body(json!({"projectionDeleted": true}))
        .send()
        .await?
        .error_for_status_code()?;
    client
        .delete(DeleteParts::IndexId(DEFAULT_INDEX, &control_id))
        .version(3)
        .version_type(VersionType::External)
        .send()
        .await?
        .error_for_status_code()?;
    let control_write = client
        .index(IndexParts::IndexId(DEFAULT_INDEX, &control_id))
        .version(2)
        .version_type(VersionType::External)
        .body(json!({"projectionDeleted": true}));
    tokio::time::sleep(Duration::from_secs(65)).await;
    assert_eq!(
        StatusCode::CREATED,
        control_write.send().await?.status_code()
    );

    assert_eq!(StatusCode::CONFLICT, paused.resume().await?);
    assert_eq!(
        ProductListingSearchProjectionWriteOutcome::Stale,
        delayed.await??
    );
    assert_eq!(StatusCode::CONFLICT, paused_unseen.resume().await?);
    assert_eq!(
        ProductListingSearchProjectionWriteOutcome::Stale,
        delayed_unseen.await??
    );
    for id in [source.product_listing_id, unseen_id] {
        let stored = stored(id).await?;
        assert_eq!(json!(3), stored["_version"]);
        assert_eq!(
            json!({"productListingId": id, "projectionDeleted": true}),
            stored["_source"]
        );
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Stale,
            writer.delete(id, 3).await?
        );
    }
    assert_visible(&source, 0).await?;

    // A delayed withdrawal cannot remove a subsequent legitimate restoration.
    let mut paused_delete = PausedWrite::new(client.clone()).await?;
    let stale_deleter = OpenSearchProductListingSearchProjection::new(paused_delete.client.clone());
    let id = source.product_listing_id;
    let delayed_delete = tokio::spawn(async move { stale_deleter.delete(id, 4).await });
    paused_delete.wait_until_received().await?;
    source.projection_version = 5;
    source.event_id = EventId::new();
    source.current_event_id = source.event_id;
    assert_eq!(
        ProductListingSearchProjectionWriteOutcome::Applied,
        writer.upsert(&source, None).await?
    );
    assert_eq!(StatusCode::CONFLICT, paused_delete.resume().await?);
    assert_eq!(
        ProductListingSearchProjectionWriteOutcome::Stale,
        delayed_delete.await??
    );
    assert_eq!(
        ProductListingSearchProjectionWriteOutcome::Stale,
        writer.upsert(&source, None).await?
    );
    let restored = stored(source.product_listing_id).await?;
    assert_eq!(json!(5), restored["_version"]);
    assert_eq!(
        serde_json::to_value(product_listing_document(&source, None)?)?,
        restored["_source"]
    );
    assert_visible(&source, 1).await?;
    Ok(())
}

async fn stored(id: ProductListingId) -> TestResult<Value> {
    Ok(get_opensearch_client()
        .await
        .get(GetParts::IndexId(DEFAULT_INDEX, &id.to_string()))
        .send()
        .await?
        .error_for_status_code()?
        .json()
        .await?)
}

async fn assert_visible(
    source: &ProductListingSearchFilterMatchSource,
    expected: usize,
) -> TestResult {
    refresh_index(DEFAULT_INDEX).await;
    let client = get_opensearch_client().await.clone();
    let snapshot = NewFxRateSnapshot::capture_eur(
        FxRateId::new(),
        OffsetDateTime::UNIX_EPOCH,
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
    )?
    .into_persisted(1_i64.try_into()?);
    let plan = ProductListingPriceFilterPlan::compile(snapshot, Currency::Eur, None)?;
    let request = ProductListingSearchReadRequest {
        compiled_search: CompiledProductListingSearch {
            search: ProductListingSearch::new(Language::En, Currency::Eur),
            price_filter_plan: plan.clone(),
        },
        sort: None,
        cursor: None,
    };
    let reader = OpenSearchProductListingSearchReader::new(client.clone());
    let ordinary = reader.search(&request).await?;
    assert_eq!(expected, ordinary.items.len());
    assert_eq!(Some(expected as u64), ordinary.total);
    let embedding = vec![0.1; 768];
    let hybrid = reader.search_hybrid(&request, &embedding).await?;
    assert_eq!(expected, hybrid.items.len());
    let similar = OpenSearchProductListingSimilarProductListingsReader::new(client)
        .find_similar_product_listings(&ProductListingSimilarProductListingsRequest {
            product_listing_id: ProductListingId::new(),
            embedding,
            language: Language::En,
            price_filter_plan: plan,
        })
        .await?;
    assert_eq!(expected, similar.len());
    for item in ordinary.items.iter().chain(&hybrid.items).chain(&similar) {
        assert_eq!(source.product_listing_id, item.product_listing_id);
        assert_eq!(source.current_event_id, item.event_id);
    }
    Ok(())
}

fn source() -> TestResult<ProductListingSearchFilterMatchSource> {
    let event_id = EventId::new();
    Ok(ProductListingSearchFilterMatchSource {
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
        product_title: Some(Localized::new(Language::En, Title::from("Fenced listing"))),
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
        embedding: Some(
            std::iter::once(1.0)
                .chain(std::iter::repeat_n(0.0, 767))
                .collect(),
        ),
        auction: Default::default(),
        created: OffsetDateTime::UNIX_EPOCH,
        updated: OffsetDateTime::UNIX_EPOCH,
    })
}

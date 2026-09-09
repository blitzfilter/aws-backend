use application::pagination::Cursor;
use domain_primitives::event_id::EventId;
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use fxrate_core::{
    FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot,
};
use indexmap::IndexSet;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount};
use product_listing_core::{
    listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle,
    listing_orderability::ListingOrderability,
    product_listing::{
        ProductListingAuction, ProductListingPriceValuationBasis, ProductListingPricing,
    },
    product_listing_image::ProductListingImage,
    product_listing_search::{ListingAvailabilityQuery, ProductListingSearch},
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_service::ports::{
    ListingSourceSummary, ProductListingPercolationInput, ProductListingPercolationValuation,
    ProductListingPricesByCurrency, ProductListingSearchFilterMatchSource,
};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterIndexQuery, SearchFilterProjection,
    SearchFilterProjectionWriteOutcome, SearchFilterView,
};
use serde_json::json;
use std::collections::HashMap;
use strum::IntoEnumIterator;
use user_core::user_id::UserId;

use test_api::{
    IntegrationTestService, OpenSearch, aura_integration_test, get_opensearch_client, refresh_index,
};
use time::{OffsetDateTime, macros::datetime};
use url::Url;

#[aura_integration_test(services = [OpenSearch()])]
async fn should_index_query_percolate_and_delete_search_filter_document() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let view = sample_view("canonical opensearch renaissance cabinet");

    index
        .upsert(&projection(&view, 1))
        .await
        .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    refresh_index("user_search_filters").await;

    let query_result = index
        .query(&SearchFilterIndexQuery {
            state: Some(SearchFilterState::Active),
            has_enhanced_search_description: Some(false),
            cursor: Some(Cursor {
                size: 50,
                search_after: None,
            }),
        })
        .await
        .unwrap_or_else(|error| panic!("query failed: {error:?}"));
    assert_eq!(
        Some(&json!([view.search_filter_id.to_string()])),
        query_result.cursor.search_after.as_ref()
    );
    let indexed = match query_result
        .items
        .iter()
        .find(|item| item.search_filter_id == view.search_filter_id)
    {
        Some(item) => item,
        None => panic!("indexed search filter was not returned"),
    };
    assert_eq!(view.search, indexed.search);

    let matching_product = percolation_input("canonical opensearch renaissance cabinet");
    let percolate_result = index
        .percolate(&matching_product)
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        percolate_result
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );

    index
        .delete(view.search_filter_id, 2)
        .await
        .unwrap_or_else(|error| panic!("delete failed: {error:?}"));
    assert!(matches!(
        index.upsert(&projection(&view, 1)).await,
        Ok(SearchFilterProjectionWriteOutcome::Stale)
    ));
    refresh_index("user_search_filters").await;

    let after_delete = index
        .percolate(&matching_product)
        .await
        .unwrap_or_else(|error| panic!("percolate after delete failed: {error:?}"));
    assert!(
        !after_delete
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_percolate_any_of_multiple_product_queries() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let view = SearchFilterView {
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query(text_query("copper table lamp"))
            .with_product_listing_query(text_query("bronze floor lamp")),
        ..sample_view("copper table lamp")
    };

    index
        .upsert(&projection(&view, 1))
        .await
        .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    refresh_index("user_search_filters").await;

    let matching_product = percolation_input("bronze floor lamp");
    let matches = index
        .percolate(&matching_product)
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        matches
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );

    let non_matching_product = percolation_input("garden sculpture");
    let non_matches = index
        .percolate(&non_matching_product)
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        !non_matches
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_percolate_a_real_filter_covering_every_product_listing_field() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let matching_product = maximal_percolation_input()
        .unwrap_or_else(|error| panic!("maximal product input failed: {error}"));
    let source = &matching_product.source;
    let view = SearchFilterView {
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query(text_query("maximal percolation cabinet"))
            .with_enhanced_search_description(
                "maximal percolation cabinet"
                    .try_into()
                    .unwrap_or_else(|error| panic!("invalid enhanced search description: {error}")),
            )
            .with_exclude_product_listing_id_query(
                std::collections::HashSet::from([
                    product_listing_core::product_listing_id::ProductListingId::new(),
                ])
                .into(),
            )
            .with_listing_source_id_query(
                std::collections::HashSet::from([source.source.listing_source_id]).into(),
            )
            .with_exclude_listing_source_id_query(
                std::collections::HashSet::from([ListingSourceId::new()]).into(),
            )
            .with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(12_000_u64)),
                max: Some(MonetaryAmount::from(13_000_u64)),
            })
            .with_availability_query(ListingAvailabilityQuery {
                any_of: std::collections::HashSet::from([ListingAvailability::InStock]).into(),
                orderability: std::collections::HashSet::from([ListingOrderability::OrderableNow])
                    .into(),
                include_unspecified: true,
            })
            .with_created_query(RangeQuery {
                min: Some(datetime!(2026-01-01 00:00:00 UTC)),
                max: Some(datetime!(2026-01-01 00:00:00 UTC)),
            })
            .with_updated_query(RangeQuery {
                min: Some(datetime!(2026-01-02 00:00:00 UTC)),
                max: Some(datetime!(2026-01-02 00:00:00 UTC)),
            })
            .with_auction_start_query(RangeQuery {
                min: Some(datetime!(2026-01-03 00:00:00 UTC)),
                max: Some(datetime!(2026-01-03 00:00:00 UTC)),
            })
            .with_auction_end_query(RangeQuery {
                min: Some(datetime!(2026-01-04 00:00:00 UTC)),
                max: Some(datetime!(2026-01-04 00:00:00 UTC)),
            }),
        ..sample_view("maximal percolation cabinet")
    };

    index
        .upsert(&projection(&view, 1))
        .await
        .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    refresh_index("user_search_filters").await;

    let matches = index
        .percolate(&matching_product)
        .await
        .unwrap_or_else(|error| panic!("percolate failed: {error:?}"));
    assert!(
        matches
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id),
        "maximal product did not match its all-fields saved filter"
    );
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_use_application_default_size_for_first_query_page() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let views: Vec<_> = (0..25)
        .map(|number| sample_view(&format!("application page size filter {number}")))
        .collect();

    for view in &views {
        index
            .upsert(&projection(view, 1))
            .await
            .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    }
    refresh_index("user_search_filters").await;

    let result = index
        .query(&SearchFilterIndexQuery {
            state: Some(SearchFilterState::Active),
            has_enhanced_search_description: Some(false),
            ..Default::default()
        })
        .await
        .unwrap_or_else(|error| panic!("query failed: {error:?}"));

    assert_eq!(21, result.cursor.size);
    assert_eq!(21, result.items.len());
    assert!(result.cursor.search_after.is_some());
}

#[aura_integration_test(services = [OpenSearch()])]
async fn should_filter_query_by_enhanced_description_presence() {
    let client = get_opensearch_client().await.clone();
    let index = OpenSearchSearchFilterIndex::new(client);
    let view = sample_view_with_enhanced_description("canonical enhanced brass lamp");

    index
        .upsert(&projection(&view, 1))
        .await
        .unwrap_or_else(|error| panic!("index failed: {error:?}"));
    refresh_index("user_search_filters").await;

    let query_result = index
        .query(&SearchFilterIndexQuery {
            state: Some(SearchFilterState::Active),
            has_enhanced_search_description: Some(true),
            cursor: Some(Cursor {
                size: 50,
                search_after: None,
            }),
        })
        .await
        .unwrap_or_else(|error| panic!("query failed: {error:?}"));

    assert!(
        query_result
            .items
            .iter()
            .any(|item| item.search_filter_id == view.search_filter_id)
    );
}

fn maximal_percolation_input() -> Result<ProductListingPercolationInput, Box<dyn std::error::Error>>
{
    let snapshot = NewFxRateSnapshot::capture_eur(
        FxRateId::new(),
        datetime!(2026-01-01 00:00:00 UTC),
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Eur {
                    FX_RATE_SCALE
                } else {
                    1_000_000
                },
            )
        }),
    )?
    .into_persisted(FxRateGeneration::try_from(1)?);
    let source_price = money::Price::new(12_500_u64.into(), Currency::Eur);
    let mut source = product_source("maximal percolation cabinet");
    source.product_title = Some(Localized::new(
        Language::En,
        Title::from("maximal percolation cabinet"),
    ));
    source.titles = HashMap::from([
        (Language::De, Title::from("maximales Perkolationskabinett")),
        (Language::En, Title::from("maximal percolation cabinet")),
        (Language::Fr, Title::from("cabinet de percolation maximal")),
        (Language::Es, Title::from("gabinete de percolación máximo")),
        (Language::It, Title::from("mobile di percolazione massimo")),
    ]);
    source.pricing.price = Some(source_price);
    source.availability = Some(ListingAvailability::InStock);
    source.images = IndexSet::from([ProductListingImage::new(Url::parse(
        "https://shop.example.test/product_listings/sku-1/image.jpg",
    )?)]);
    source.auction = ProductListingAuction {
        start: Some(datetime!(2026-01-03 00:00:00 UTC)),
        end: Some(datetime!(2026-01-04 00:00:00 UTC)),
    };
    source.created = datetime!(2026-01-01 00:00:00 UTC);
    source.updated = datetime!(2026-01-02 00:00:00 UTC);

    Ok(ProductListingPercolationInput {
        valuation: Some(ProductListingPercolationValuation {
            basis: ProductListingPriceValuationBasis::Event,
            fx_rate_id: snapshot.id(),
            effective_at: snapshot.captured_at(),
            prices: ProductListingPricesByCurrency::convert_all(&snapshot, source_price)?,
        }),
        source,
    })
}

fn percolation_input(title: &str) -> ProductListingPercolationInput {
    ProductListingPercolationInput {
        source: product_source(title),
        valuation: None,
    }
}

fn product_source(title: &str) -> ProductListingSearchFilterMatchSource {
    let event_id = EventId::new();
    ProductListingSearchFilterMatchSource {
        event_id,
        event_kind:
            product_listing_service::ports::ProductListingSearchFilterMatchSourceEventKind::Domain,
        origin_event_time: OffsetDateTime::UNIX_EPOCH,
        current_event_id: event_id,
        projection_version: 1,
        product_listing_id: product_listing_core::product_listing_id::ProductListingId::new(),
        product_listing_title_slug_id:
            product_listing_core::product_listing_slug_id::ProductListingSlugId::raw(
                "product-a1b2c3",
            )
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
        source: ListingSourceSummary {
            listing_source_id: ListingSourceId::new(),
            name: ListingSourceName::try_from("Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            slug_id: ListingSourceSlugId::raw("source")
                .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
        },
        source_listing_id: SourceListingId::try_from("sku-1")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),

        product_title: None,
        product_description: None,
        titles: HashMap::from([(Language::En, Title::from(title))]),
        descriptions: HashMap::new(),
        pricing: ProductListingPricing::default(),
        sale_observation: None,
        availability: Some(ListingAvailability::Available),
        lifecycle: ListingLifecycle::Active,
        url: Url::parse("https://shop.example.test/product_listings/sku-1")
            .unwrap_or_else(|error| panic!("test URL must be valid: {error}")),
        view_url: Url::parse("https://aura.example.test/product_listings/product")
            .unwrap_or_else(|error| panic!("test URL must be valid: {error}")),
        image: None,
        images: IndexSet::<ProductListingImage>::new(),
        embedding: None,
        auction: ProductListingAuction::default(),
        created: OffsetDateTime::UNIX_EPOCH,
        updated: OffsetDateTime::UNIX_EPOCH,
    }
}

fn projection(view: &SearchFilterView, source_version: i64) -> SearchFilterProjection {
    SearchFilterProjection {
        view: view.clone(),
        source_version,
    }
}

fn sample_view(query_text: &str) -> SearchFilterView {
    SearchFilterView {
        search_filter_id: UserSearchFilterId::new(),
        user_id: UserId::new(),
        name: UserSearchFilterName::from("canonical search"),
        notifications: true,
        state: SearchFilterState::Active,
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query(text_query(query_text)),
        embedding: None,
        created: datetime!(2026-01-01 00:00:00 UTC),
        updated: datetime!(2026-01-01 00:00:00 UTC),
    }
}

fn sample_view_with_enhanced_description(query_text: &str) -> SearchFilterView {
    SearchFilterView {
        search: ProductListingSearch::new(Language::En, Currency::Eur)
            .with_product_listing_query(text_query(query_text))
            .with_enhanced_search_description(
                product_listing_core::product_listing_search::EnhancedSearchDescription::try_from(
                    "brass lamp",
                )
                .unwrap(),
            ),
        ..sample_view(query_text)
    }
}

fn text_query(value: &str) -> TextQuery<1> {
    match value.try_into() {
        Ok(query) => query,
        Err(error) => panic!("invalid text query: {error:?}"),
    }
}

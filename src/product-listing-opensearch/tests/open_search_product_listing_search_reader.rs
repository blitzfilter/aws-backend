use application::pagination::Cursor;
use domain_primitives::event_id::EventId;
use domain_primitives::query::range_query::RangeQuery;
use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;
use money::Currency;
use money::MonetaryAmount;
use opensearch::IndexParts;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_service::ports::{
    CompiledProductListingSearch, ProductListingPriceFilterPlan, ProductListingSearchReadRequest,
    ProductListingSearchReader,
};
use product_listing_service::use_cases::queries::search_product_listings::ProductListingSearchReadResult;
use serde_json::{Value, json};

use std::io::{Error as IoError, ErrorKind};
use strum::IntoEnumIterator;
use test_api::{
    IntegrationTestService, OpenSearch, aura_integration_test, get_opensearch_client,
    refresh_index, serial,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const PRODUCTS_INDEX: &str = "product-listings";

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [OpenSearch()])]
#[serial]
async fn should_search_active_and_sold_products_with_one_pinned_price_plan() {
    assert_ok(should_search_active_and_sold_products_with_one_pinned_price_plan_impl().await);
}

async fn should_search_active_and_sold_products_with_one_pinned_price_plan_impl() -> TestResult {
    let active = product_listing_document(ProductListingSeed::active("active", 100))?;
    let sold = product_listing_document(ProductListingSeed::sold("sold", 110))?;
    index_products([active.document, sold.document]).await?;

    let result = search(
        ProductListingSearch::new(localization::Language::En, Currency::Usd).with_price_query(
            RangeQuery {
                min: Some(MonetaryAmount::from(110_u64)),
                max: Some(MonetaryAmount::from(110_u64)),
            },
        ),
        price_filter(Some(RangeQuery {
            min: Some(MonetaryAmount::from(110_u64)),
            max: Some(MonetaryAmount::from(110_u64)),
        }))?,
    )
    .await?;

    assert_eq!(Some(2), result.total);
    assert_eq!(2, result.items.len());
    let tie_break = result
        .cursor
        .search_after
        .as_ref()
        .and_then(Value::as_array)
        .and_then(|sort| sort.last())
        .and_then(Value::as_str)
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "typed ID sort value missing"))?;
    let parsed_tie_break = tie_break.parse::<ProductListingId>()?;
    assert_eq!(tie_break, parsed_tie_break.to_string());
    assert!(result.items.iter().all(|item| {
        item.display_price
            == Some(money::Price::new(
                MonetaryAmount::from(110_u64),
                Currency::Usd,
            ))
    }));
    Ok(())
}

#[aura_integration_test(services = [OpenSearch()])]
#[serial]
async fn should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search()
 {
    assert_ok(
        should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search_impl()
            .await,
    );
}

async fn should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search_impl()
-> TestResult {
    let sold_without_price =
        product_listing_document(ProductListingSeed::sold_without_main_price("sold-no-price"))?;
    let product_listing_id = sold_without_price.document["productListingId"]
        .as_str()
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "productListingId missing"))?
        .to_owned();
    index_products([sold_without_price.document]).await?;

    let non_price_result = search(
        ProductListingSearch::new(localization::Language::En, Currency::Usd),
        price_filter(None)?,
    )
    .await?;
    assert_eq!(Some(1), non_price_result.total);
    assert_eq!(1, non_price_result.items.len());
    assert_eq!(
        product_listing_id,
        non_price_result.items[0].product_listing_id.to_string()
    );
    assert_eq!(None, non_price_result.items[0].display_price);
    assert!(matches!(
        non_price_result.items[0].price_valuation,
        product_listing_service::use_cases::ProductListingSummaryPriceValuation::SaleObservation { .. }
    ));

    let price_result = search(
        ProductListingSearch::new(localization::Language::En, Currency::Usd).with_price_query(
            RangeQuery {
                min: Some(MonetaryAmount::from(1_u64)),
                max: Some(MonetaryAmount::from(1_000_u64)),
            },
        ),
        price_filter(Some(RangeQuery {
            min: Some(MonetaryAmount::from(1_u64)),
            max: Some(MonetaryAmount::from(1_000_u64)),
        }))?,
    )
    .await?;
    assert_eq!(Some(0), price_result.total);
    assert!(price_result.items.is_empty());
    Ok(())
}

async fn search(
    search: ProductListingSearch,
    price_filter: ProductListingPriceFilterPlan,
) -> Result<
    ProductListingSearchReadResult,
    product_listing_service::ports::ProductListingSearchReadError,
> {
    let reader = product_listing_opensearch::OpenSearchProductListingSearchReader::with_index(
        get_opensearch_client().await.clone(),
        PRODUCTS_INDEX,
    );
    reader
        .search(&ProductListingSearchReadRequest {
            compiled_search: CompiledProductListingSearch {
                search,
                price_filter_plan: price_filter,
            },
            sort: None,
            cursor: Some(Cursor {
                size: 20,
                search_after: None,
            }),
        })
        .await
}

fn price_filter(
    range: Option<RangeQuery<MonetaryAmount>>,
) -> Result<ProductListingPriceFilterPlan, Box<dyn std::error::Error + Send + Sync>> {
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};

    let snapshot = NewFxRateSnapshot::capture_eur(
        FxRateId::new(),
        OffsetDateTime::UNIX_EPOCH,
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Usd {
                    1_100_000
                } else {
                    FX_RATE_SCALE
                },
            )
        }),
    )?
    .into_persisted(1_i64.try_into()?);
    Ok(ProductListingPriceFilterPlan::compile(
        snapshot,
        Currency::Usd,
        range,
    )?)
}

async fn index_products(product_listings: impl IntoIterator<Item = Value>) -> TestResult {
    let client = get_opensearch_client().await;
    for product in product_listings {
        let id = product_listing_id_string(&product)?;
        let response = client
            .index(IndexParts::IndexId(PRODUCTS_INDEX, &id))
            .body(product)
            .send()
            .await?;
        let status = response.status_code();
        let payload = response.text().await?;
        if !status.is_success() {
            return Err(IoError::new(
                ErrorKind::InvalidData,
                format!("failed indexing product {id}: HTTP {status}: {payload}"),
            )
            .into());
        }
    }
    refresh_index(PRODUCTS_INDEX).await;
    Ok(())
}

fn product_listing_id_string(product: &Value) -> Result<String, IoError> {
    product
        .get("productListingId")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "productListingId missing"))
}

fn assert_ok(result: TestResult) {
    assert_eq!(Ok(()), result.map_err(|error| error.to_string()));
}

struct ProductListingSeed {
    product_listing_id: ProductListingId,
    product_listing_title_slug_id: ProductListingSlugId,
    source_price: Option<u64>,
    sale_price: Option<u64>,
    has_sale_observation: bool,
}

impl ProductListingSeed {
    fn active(slug: &str, source_price: u64) -> Self {
        Self {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::from_title_and_suffix(
                slug, "a1b2c3",
            )
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source_price: Some(source_price),
            sale_price: None,
            has_sale_observation: false,
        }
    }

    fn sold(slug: &str, sale_price: u64) -> Self {
        Self {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::from_title_and_suffix(
                slug, "a1b2c3",
            )
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source_price: None,
            sale_price: Some(sale_price),
            has_sale_observation: true,
        }
    }

    fn sold_without_main_price(slug: &str) -> Self {
        Self {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::from_title_and_suffix(
                slug, "a1b2c3",
            )
            .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            source_price: None,
            sale_price: None,
            has_sale_observation: true,
        }
    }
}

struct IndexedProductListing {
    document: Value,
}

fn product_listing_document(
    seed: ProductListingSeed,
) -> Result<IndexedProductListing, time::error::Format> {
    let sale_prices = seed.sale_price.map(|amount| {
        json!({
            "eur": amount, "gbp": amount, "usd": amount, "aud": amount,
            "cad": amount, "nzd": amount, "cny": amount, "brl": amount,
            "pln": amount, "try": amount, "jpy": amount, "czk": amount,
            "rub": amount, "aed": amount, "sar": amount, "hkd": amount,
            "sgd": amount, "chf": amount, "zar": amount
        })
    });
    let sale_observed_at = seed
        .has_sale_observation
        .then(|| OffsetDateTime::UNIX_EPOCH.format(&Rfc3339))
        .transpose()?;
    let document = json!({
        "productListingId": seed.product_listing_id,
        "productListingTitleSlugId": seed.product_listing_title_slug_id,
        "listingSourceId": ListingSourceId::new().to_string(),
        "sourceListingId": SourceListingId::try_from("sku-1")
                    .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
        "eventId": EventId::new(),
        "title": { "text": "Blue vase", "language": "EN" },
        "titleEn": "Blue vase",
        "sourcePrice": seed.source_price.map(|amount| json!({ "amount": amount, "currency": "EUR" })),
        "salePrices": sale_prices,
        "saleObservationFxRateId": seed.has_sale_observation.then(FxRateId::new),
        "saleObservedAt": sale_observed_at,

        "url": format!("https://shop.example/product_listings/{}", seed.product_listing_title_slug_id),
        "created": OffsetDateTime::UNIX_EPOCH.format(&Rfc3339)?,
        "updated": OffsetDateTime::UNIX_EPOCH.format(&Rfc3339)?
    });

    Ok(IndexedProductListing { document })
}

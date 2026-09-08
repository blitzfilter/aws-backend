use crate::product_listing_document::{ProductListingDocument, ProductListingDocumentSerdeField};
use crate::product_listing_search_reader::map_search_response;
use application::error::box_error;
use opensearch::{OpenSearch, SearchParts};
use platform_opensearch::{response::read_response, search_response::SearchResponse};
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_service::ports::{
    CompiledProductListingSearch, ProductListingSimilarProductListingsReadError,
    ProductListingSimilarProductListingsReader, ProductListingSimilarProductListingsRequest,
};

use serde_json::json;

const DEFAULT_INDEX: &str = "product-listings";
const DEFAULT_RESULT_COUNT: u64 = 20;

#[derive(Clone)]
pub struct OpenSearchProductListingSimilarProductListingsReader {
    client: OpenSearch,
    index: String,
}

impl OpenSearchProductListingSimilarProductListingsReader {
    pub fn new(client: OpenSearch) -> Self {
        Self {
            client,
            index: DEFAULT_INDEX.to_owned(),
        }
    }

    pub fn with_index(client: OpenSearch, index: impl Into<String>) -> Self {
        Self {
            client,
            index: index.into(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingSimilarProductListingsReader
    for OpenSearchProductListingSimilarProductListingsReader
{
    #[tracing::instrument(name = "opensearch_product_similar_products", skip_all)]
    async fn find_similar_product_listings(
        &self,
        request: &ProductListingSimilarProductListingsRequest,
    ) -> Result<
        Vec<product_listing_service::use_cases::ProductListingSearchItem>,
        ProductListingSimilarProductListingsReadError,
    > {
        let response = self
            .client
            .search(SearchParts::Index(&[self.index.as_str()]))
            .body(build_similar_products_request(request))
            .send()
            .await
            .map_err(|error| {
                ProductListingSimilarProductListingsReadError::SimilarProductListingsQueryFailed {
                    source: box_error(error),
                }
            })?;
        let response = read_response(response).await.map_err(|error| {
            ProductListingSimilarProductListingsReadError::SimilarProductListingsQueryFailed {
                source: box_error(error),
            }
        })?;
        if !response.status().is_success() {
            return Err(
                ProductListingSimilarProductListingsReadError::SimilarProductListingsQueryFailed {
                    source: box_error(std::io::Error::other(format!(
                        "OpenSearch similar product listings returned {}: {}",
                        response.status(),
                        response.body(),
                    ))),
                },
            );
        }
        let payload = response.into_body();
        let search_response = serde_json::from_str::<SearchResponse<ProductListingDocument>>(
            &payload,
        )
        .map_err(|error| {
            ProductListingSimilarProductListingsReadError::SimilarProductListingsQueryFailed {
                source: box_error(error),
            }
        })?
        .into_non_timed_out("similar product_listings")
        .map_err(|error| {
            ProductListingSimilarProductListingsReadError::SimilarProductListingsQueryFailed {
                source: box_error(error),
            }
        })?;
        let search =
            ProductListingSearch::new(request.language, request.price_filter_plan.target_currency);
        let compiled_search = CompiledProductListingSearch {
            search,
            price_filter_plan: request.price_filter_plan.clone(),
        };

        map_search_response(&compiled_search, search_response)
            .map(|result| result.items)
            .map_err(|error| {
                ProductListingSimilarProductListingsReadError::SimilarProductListingsQueryFailed {
                    source: box_error(error),
                }
            })
    }
}

pub(crate) fn build_similar_products_request(
    request: &ProductListingSimilarProductListingsRequest,
) -> serde_json::Value {
    json!({
        "_source": { "excludes": [ProductListingDocumentSerdeField::Embedding] },
        "size": DEFAULT_RESULT_COUNT,
        "query": {
            "knn": {
                ProductListingDocumentSerdeField::Embedding.as_str(): {
                    "vector": request.embedding,
                    "k": DEFAULT_RESULT_COUNT,
                    "filter": {
                        "bool": {
                            "must_not": [{
                                "term": {
                                    ProductListingDocumentSerdeField::ProductListingId.as_str(): request.product_listing_id.to_string()
                                }
                            }, {"term": {"projectionDeleted": true}}]
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxrate_core::{FX_RATE_SCALE, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use localization::Language;
    use money::Currency;
    use product_listing_core::product_listing_id::ProductListingId;
    use product_listing_service::ports::ProductListingPriceFilterPlan;
    use serde_json::json;
    use strum::IntoEnumIterator;
    use time::OffsetDateTime;

    fn test_price_filter_plan() -> ProductListingPriceFilterPlan {
        let snapshot = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )
        .and_then(|snapshot| Ok(snapshot.into_persisted(1_i64.try_into()?)))
        .unwrap_or_else(|error| panic!("test FX snapshot must be valid: {error}"));
        ProductListingPriceFilterPlan::compile(snapshot, Currency::Eur, None)
            .unwrap_or_else(|error| panic!("test plan must compile: {error}"))
    }

    #[test]
    fn should_build_knn_request_that_excludes_source_product_and_embedding() {
        let product_listing_id = ProductListingId::new();
        let request = ProductListingSimilarProductListingsRequest {
            product_listing_id,
            embedding: vec![0.1, 0.2],
            language: Language::De,
            price_filter_plan: test_price_filter_plan(),
        };

        let actual = build_similar_products_request(&request);

        assert_eq!(
            actual.pointer("/_source/excludes/0"),
            Some(&json!("embedding"))
        );
        assert_eq!(actual.pointer("/size"), Some(&json!(20)));
        assert_eq!(
            actual.pointer("/query/knn/embedding/vector"),
            Some(&json!(request.embedding))
        );
        assert_eq!(actual.pointer("/query/knn/embedding/k"), Some(&json!(20)));
        assert_eq!(
            actual.pointer("/query/knn/embedding/filter/bool/must_not/0/term/productListingId"),
            Some(&json!(product_listing_id.to_string()))
        );
        assert!(actual.pointer("/query/bool").is_none());
    }
}

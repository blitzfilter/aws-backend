use crate::document::SearchFilterDocument;
use application::error::box_error;
use application::pagination::{Cursor, CursoredResult};
use platform_opensearch::{
    response::{OpenSearchResponse, read_response},
    search_response::SearchResponse,
};

use opensearch::{
    IndexParts, OpenSearch, SearchParts,
    http::{Method, StatusCode, headers::HeaderMap, request::JsonBody},
    params::VersionType,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    SearchFilterIndex, SearchFilterIndexError, SearchFilterIndexQuery, SearchFilterProjection,
    SearchFilterProjectionWriteOutcome, SearchFilterView,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashSet;

const DEFAULT_INDEX: &str = "user_search_filters";
const PERCOLATION_PAGE_SIZE: u64 = 20;
const PIT_KEEP_ALIVE: &str = "1m";

#[derive(Debug, Deserialize)]
struct PointInTimeResponse {
    pit_id: String,
}

#[derive(Debug, thiserror::Error)]
enum PercolationCompletenessError {
    #[error("percolation response timed out")]
    TimedOut,
    #[error("percolation response has {failed_shards} failed shards")]
    FailedShards { failed_shards: u64 },
    #[error("percolation response total is not exact (relation: {relation})")]
    InexactTotal { relation: String },
    #[error("percolation PIT returned inconsistent totals ({expected} then {actual})")]
    InconsistentTotal { expected: u64, actual: u64 },
    #[error("percolation hit is missing stable sort values")]
    MissingSortValues,
    #[error("percolation search_after did not advance")]
    NonAdvancingSearchAfter,
    #[error("percolation returned {returned} distinct filters but reported {total} hits")]
    Incomplete { returned: u64, total: u64 },
}

#[derive(Clone)]
pub struct OpenSearchSearchFilterIndex {
    client: OpenSearch,
    index: String,
}

impl OpenSearchSearchFilterIndex {
    pub fn new(client: OpenSearch) -> Self {
        Self {
            client,
            index: DEFAULT_INDEX.to_owned(),
        }
    }

    async fn open_point_in_time(&self) -> Result<String, SearchFilterIndexError> {
        let path = format!("/{}/_search/point_in_time", self.index);
        let response = self
            .client
            .send(
                Method::Post,
                &path,
                HeaderMap::new(),
                Some(&[("keep_alive", PIT_KEEP_ALIVE)]),
                None::<JsonBody<serde_json::Value>>,
                None,
            )
            .await
            .map_err(percolation_error)?;
        let response = read_response(response)
            .await
            .map_err(percolation_response_read_error)?;
        if !response.status().is_success() {
            return Err(percolation_status_error(response));
        }
        let response =
            serde_json::from_str::<PointInTimeResponse>(response.body()).map_err(|source| {
                SearchFilterIndexError::PercolateFailed {
                    source: box_error(source),
                }
            })?;
        Ok(response.pit_id)
    }

    async fn close_point_in_time(&self, pit_id: &str) -> Result<(), SearchFilterIndexError> {
        let response = self
            .client
            .send(
                Method::Delete,
                "/_search/point_in_time",
                HeaderMap::new(),
                None::<&serde_json::Value>,
                Some(JsonBody::new(json!({"pit_id": [pit_id]}))),
                None,
            )
            .await
            .map_err(percolation_error)?;
        let response = read_response(response)
            .await
            .map_err(percolation_response_read_error)?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(percolation_status_error(response))
        }
    }

    async fn percolate_all(
        &self,
        product_document: &serde_json::Value,
        pit_id: &str,
    ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
        let mut matched_filter_ids = HashSet::new();
        let mut matches = Vec::new();
        let mut expected_total = None;
        let mut search_after = None;

        loop {
            let response = self
                .client
                .search(SearchParts::None)
                .body(build_percolate_body(
                    product_document,
                    pit_id,
                    search_after.as_ref(),
                ))
                .send()
                .await
                .map_err(percolation_error)?;
            let response = read_response(response)
                .await
                .map_err(percolation_response_read_error)?;
            if !response.status().is_success() {
                return Err(percolation_status_error(response));
            }
            let response =
                serde_json::from_str::<SearchResponse<SearchFilterDocument>>(response.body())
                    .map_err(|source| SearchFilterIndexError::InvalidDocument {
                        source: box_error(source),
                    })?;
            let total = complete_percolation_total(&response)?;

            if let Some(expected) = expected_total {
                if expected != total {
                    return Err(percolation_completeness_error(
                        PercolationCompletenessError::InconsistentTotal {
                            expected,
                            actual: total,
                        },
                    ));
                }
            } else {
                expected_total = Some(total);
            }

            if response.hits.hits.is_empty() {
                break;
            }

            let next_search_after = response
                .hits
                .hits
                .last()
                .and_then(|hit| hit.sort.clone())
                .ok_or_else(|| {
                    percolation_completeness_error(PercolationCompletenessError::MissingSortValues)
                })?;
            if search_after.as_ref() == Some(&next_search_after) {
                return Err(percolation_completeness_error(
                    PercolationCompletenessError::NonAdvancingSearchAfter,
                ));
            }
            search_after = Some(next_search_after);

            for hit in response.hits.hits {
                let view = SearchFilterView::try_from(hit.source).map_err(|source| {
                    SearchFilterIndexError::InvalidDocument {
                        source: box_error(source),
                    }
                })?;
                if matched_filter_ids.insert(view.search_filter_id) {
                    matches.push(view);
                }
            }
        }

        let total = expected_total.unwrap_or(0);
        let returned = matched_filter_ids.len() as u64;
        if returned != total {
            return Err(percolation_completeness_error(
                PercolationCompletenessError::Incomplete { returned, total },
            ));
        }

        Ok(matches)
    }
}

fn complete_percolation_total(
    response: &SearchResponse<SearchFilterDocument>,
) -> Result<u64, SearchFilterIndexError> {
    if response.timed_out {
        return Err(percolation_completeness_error(
            PercolationCompletenessError::TimedOut,
        ));
    }
    if response.shards.failed != 0 {
        return Err(percolation_completeness_error(
            PercolationCompletenessError::FailedShards {
                failed_shards: response.shards.failed,
            },
        ));
    }
    if response.hits.total.relation != "eq" {
        return Err(percolation_completeness_error(
            PercolationCompletenessError::InexactTotal {
                relation: response.hits.total.relation.clone(),
            },
        ));
    }
    Ok(response.hits.total.value)
}

fn percolation_completeness_error(source: PercolationCompletenessError) -> SearchFilterIndexError {
    SearchFilterIndexError::PercolateFailed {
        source: box_error(source),
    }
}

fn percolation_error(source: opensearch::Error) -> SearchFilterIndexError {
    SearchFilterIndexError::PercolateFailed {
        source: box_error(source),
    }
}

fn percolation_response_read_error(
    source: platform_opensearch::response::OpenSearchResponseReadError,
) -> SearchFilterIndexError {
    SearchFilterIndexError::PercolateFailed {
        source: box_error(source),
    }
}

fn percolation_status_error(response: OpenSearchResponse) -> SearchFilterIndexError {
    SearchFilterIndexError::PercolateFailed {
        source: box_error(std::io::Error::other(format!(
            "OpenSearch percolation returned {}: {}",
            response.status(),
            response.body(),
        ))),
    }
}

fn build_percolate_body(
    product_document: &serde_json::Value,
    pit_id: &str,
    search_after: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut body = json!({
        "pit": {"id": pit_id, "keep_alive": PIT_KEEP_ALIVE},
        "query": {"bool": {
            "must": [{"percolate": {"field": "query", "document": product_document}}],
            "must_not": [{"term": {"projectionDeleted": true}}]
        }},
        "size": PERCOLATION_PAGE_SIZE,
        "track_total_hits": true,
        "sort": [{"userSearchFilterId": {"order": "asc"}}]
    });
    if let Some(search_after) = search_after {
        body["search_after"] = search_after.clone();
    }
    body
}

#[async_trait::async_trait]
impl SearchFilterIndex for OpenSearchSearchFilterIndex {
    async fn upsert(
        &self,
        projection: &SearchFilterProjection,
    ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
        let document = SearchFilterDocument::try_from(projection).map_err(|source| {
            SearchFilterIndexError::WriteFailed {
                source: box_error(source),
            }
        })?;
        let response = self
            .client
            .index(IndexParts::IndexId(
                &self.index,
                &document.user_search_filter_id.to_string(),
            ))
            .version(document.source_version)
            .version_type(VersionType::External)
            .body(document)
            .send()
            .await
            .map_err(|source| SearchFilterIndexError::WriteFailed {
                source: box_error(source),
            })?;
        projection_write_outcome(response, |source| SearchFilterIndexError::WriteFailed {
            source,
        })
        .await
    }

    async fn delete(
        &self,
        id: UserSearchFilterId,
        source_version: i64,
    ) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
        let response = self
            .client
            .index(IndexParts::IndexId(&self.index, &id.to_string()))
            .version(source_version)
            .version_type(VersionType::External)
            // Physical DELETE loses its fence after index.gc_deletes (default 60s).
            // Replace the full document, including its percolator query, with a durable fence.
            .body(json!({
                "userSearchFilterId": id,
                "sourceVersion": source_version,
                "projectionDeleted": true,
            }))
            .send()
            .await
            .map_err(|source| SearchFilterIndexError::DeleteFailed {
                source: box_error(source),
            })?;
        projection_write_outcome(response, |source| SearchFilterIndexError::DeleteFailed {
            source,
        })
        .await
    }

    async fn percolate(
        &self,
        input: &product_listing_service::ports::ProductListingPercolationInput,
    ) -> Result<Vec<SearchFilterView>, SearchFilterIndexError> {
        let product_document = product_listing_opensearch::product_listing_percolation_document(
            input,
        )
        .map_err(|source| SearchFilterIndexError::PercolateFailed {
            source: box_error(source),
        })?;
        let pit_id = self.open_point_in_time().await?;
        let percolation_result = self.percolate_all(&product_document, &pit_id).await;
        let close_result = self.close_point_in_time(&pit_id).await;

        match (percolation_result, close_result) {
            (Ok(matches), Ok(())) => Ok(matches),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    async fn query(
        &self,
        query: &SearchFilterIndexQuery,
    ) -> Result<CursoredResult<SearchFilterView, serde_json::Value>, SearchFilterIndexError> {
        let body = build_query_body(query);
        let response = self
            .client
            .search(SearchParts::Index(&[&self.index]))
            .body(body)
            .send()
            .await
            .map_err(|source| SearchFilterIndexError::QueryFailed {
                source: box_error(source),
            })?;
        let response = read_response(response).await.map_err(|source| {
            SearchFilterIndexError::QueryFailed {
                source: box_error(source),
            }
        })?;
        if !response.status().is_success() {
            return Err(SearchFilterIndexError::QueryFailed {
                source: box_error(std::io::Error::other(format!(
                    "OpenSearch search-filter query returned {}: {}",
                    response.status(),
                    response.body(),
                ))),
            });
        }
        let response = serde_json::from_str::<SearchResponse<SearchFilterDocument>>(
            response.body(),
        )
        .map_err(|source| SearchFilterIndexError::InvalidDocument {
            source: box_error(source),
        })?;
        let total = response.hits.total.value;
        let search_after = response.hits.hits.last().and_then(|hit| hit.sort.clone());
        let items: Vec<_> = response
            .hits
            .hits
            .into_iter()
            .map(|hit| SearchFilterView::try_from(hit.source))
            .collect::<Result<_, _>>()
            .map_err(|source| SearchFilterIndexError::InvalidDocument {
                source: box_error(source),
            })?;
        Ok(CursoredResult {
            cursor: Cursor {
                size: items.len() as u64,
                search_after,
            },
            items,
            total: Some(total),
        })
    }
}

async fn projection_write_outcome(
    response: opensearch::http::response::Response,
    error: impl Fn(application::error::BoxError) -> SearchFilterIndexError,
) -> Result<SearchFilterProjectionWriteOutcome, SearchFilterIndexError> {
    let response = read_response(response)
        .await
        .map_err(|source| error(box_error(source)))?;
    if response.status() == StatusCode::CONFLICT {
        return Ok(SearchFilterProjectionWriteOutcome::Stale);
    }
    if response.status().is_success() {
        return Ok(SearchFilterProjectionWriteOutcome::Applied);
    }
    Err(error(box_error(std::io::Error::other(format!(
        "OpenSearch search-filter projection returned {}: {}",
        response.status(),
        response.body(),
    )))))
}

fn build_query_body(query: &SearchFilterIndexQuery) -> serde_json::Value {
    let mut filter = Vec::new();
    if let Some(state) = query.state {
        filter.push(json!({"term": {"state": state.as_str()}}));
    }
    if let Some(has) = query.has_enhanced_search_description {
        filter.push(if has {
            json!({"exists":{"field":"search.enhancedSearchDescription"}})
        } else {
            json!({"bool":{"must_not":[{"exists":{"field":"search.enhancedSearchDescription"}}]}})
        });
    }
    let cursor = query.effective_cursor();
    let mut body = json!({
        "query":{"bool":{
            "filter":filter,
            "must_not":[{"term":{"projectionDeleted":true}}]
        }},
        "size": cursor.size,
        "sort":[{"userSearchFilterId":{"order":"asc"}}]
    });
    if let Some(search_after) = cursor.search_after {
        body["search_after"] = search_after;
    }
    body
}

#[cfg(test)]
#[path = "paused_write.rs"]
mod paused_write;
#[cfg(test)]
#[path = "projection_race_tests.rs"]
mod race_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use search_filter_core::search_filter_state::SearchFilterState;

    #[test]
    fn should_build_query_body_with_filters_and_canonical_typed_id_cursor() {
        let cursor_id = UserSearchFilterId::new();
        let body = build_query_body(&SearchFilterIndexQuery {
            state: Some(SearchFilterState::Active),
            has_enhanced_search_description: Some(true),
            cursor: Some(Cursor {
                size: 25,
                search_after: Some(json!([cursor_id.to_string()])),
            }),
        });

        assert_eq!(25, body["size"]);
        assert_eq!(json!([cursor_id.to_string()]), body["search_after"]);
        assert!(body["query"]["bool"]["filter"].is_array());
    }

    #[test]
    fn should_build_first_query_page_with_application_default_size() {
        let body = build_query_body(&SearchFilterIndexQuery::default());

        assert_eq!(Cursor::<serde_json::Value>::default().size, body["size"]);
        assert!(body.get("search_after").is_none());
        assert_eq!(
            json!([{"term": {"projectionDeleted": true}}]),
            body["query"]["bool"]["must_not"]
        );
    }

    #[test]
    fn should_build_percolation_page_with_pit_and_stable_sort() {
        let body = build_percolate_body(&json!({"title": "cabinet"}), "pit-1", None);

        assert_eq!(PERCOLATION_PAGE_SIZE, body["size"]);
        assert_eq!(true, body["track_total_hits"]);
        assert_eq!(
            json!([{"term": {"projectionDeleted": true}}]),
            body["query"]["bool"]["must_not"]
        );
        assert_eq!("pit-1", body["pit"]["id"]);
        assert_eq!(
            json!([{"userSearchFilterId": {"order": "asc"}}]),
            body["sort"]
        );
    }
}

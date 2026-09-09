//! Crawler raw ProductListing capture boundary.
//!
//! The crawler retains source-specific extraction mapping, but only writes changed immutable raw
//! revisions. Canonical ProductListing normalization and events are worker-owned.

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use listing_source_core::ListingSourceId;
use product_listing_normalization::{
    ProductListingNormalizationInput, RawProductListingProvenance,
};
use product_listing_service::ports::ProductListingRawIngestionMethod;
use product_listing_service::use_cases::{
    CaptureProductListingRawObservationCommand, CaptureProductListingRawObservationResult,
    CaptureProductListingRawObservationUseCase,
};
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, warn};
use url::Url;

/// One crawler observation prepared for durable raw capture.
#[derive(Debug, Clone)]
pub struct ProductListingRawCaptureItem {
    pub command: CaptureProductListingRawObservationCommand,
}

impl ProductListingRawCaptureItem {
    pub fn crawler(
        listing_source_id: ListingSourceId,
        candidate_url: &Url,
        input: ProductListingNormalizationInput,
        provenance: RawProductListingProvenance,
    ) -> Self {
        Self {
            command: CaptureProductListingRawObservationCommand {
                listing_source_id,
                ingestion_method: ProductListingRawIngestionMethod::WebCrawl,
                // The configured candidate URL is the crawler source-record identity.
                source_record_key: candidate_url.to_string(),
                input,
                provenance,
                source_event_id: None,
                source_occurred_at: None,
                provider_receipt: None,
            },
        }
    }
}

/// Crawler-local disposition of one capture attempt.
///
/// `DiscardedMissingSource` is terminal for this queued observation, but never claims that raw
/// business data was persisted. The next complete ListingSource sync disables stale local work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProductListingRawCaptureOutcome {
    Persisted,
    DiscardedMissingSource,
    #[default]
    RetryableFailure,
}

/// Captures input positions independently and reports durable, terminal, and retryable outcomes.
/// Results retain input order. Observations from one raw stream execute in enqueue order; only
/// independent streams run concurrently.
#[async_trait]
#[mockall::automock]
pub trait ProductListingRawCaptureService: Send + Sync {
    async fn capture(
        &self,
        observations: Vec<ProductListingRawCaptureItem>,
    ) -> Vec<ProductListingRawCaptureOutcome>;
}

/// Calls the ProductListing service raw-capture use case with bounded stream-aware concurrency.
pub struct ProductListingRawCaptureServiceImpl {
    capture_observation: Arc<dyn CaptureProductListingRawObservationUseCase>,
    max_concurrent_captures: usize,
}

impl ProductListingRawCaptureServiceImpl {
    pub fn new(
        capture_observation: Arc<dyn CaptureProductListingRawObservationUseCase>,
        max_concurrent_captures: usize,
    ) -> Self {
        Self {
            capture_observation,
            max_concurrent_captures: max_concurrent_captures.max(1),
        }
    }
}

#[async_trait]
impl ProductListingRawCaptureService for ProductListingRawCaptureServiceImpl {
    #[tracing::instrument(
        name = "crawler_raw_capture_batch",
        skip(self, observations),
        fields(total = observations.len())
    )]
    async fn capture(
        &self,
        observations: Vec<ProductListingRawCaptureItem>,
    ) -> Vec<ProductListingRawCaptureOutcome> {
        let mut results =
            vec![ProductListingRawCaptureOutcome::RetryableFailure; observations.len()];
        let mut streams = Vec::<Vec<(usize, ProductListingRawCaptureItem)>>::new();
        let mut stream_indices = HashMap::<(ListingSourceId, String), usize>::new();

        for (input_index, observation) in observations.into_iter().enumerate() {
            let stream_key = (
                observation.command.listing_source_id,
                observation.command.source_record_key.clone(),
            );
            let stream_index = match stream_indices.get(&stream_key) {
                Some(stream_index) => *stream_index,
                None => {
                    let stream_index = streams.len();
                    stream_indices.insert(stream_key, stream_index);
                    streams.push(Vec::new());
                    stream_index
                }
            };
            streams[stream_index].push((input_index, observation));
        }

        let capture_observation = Arc::clone(&self.capture_observation);
        let outcomes = stream::iter(streams.into_iter().map(|stream_items| {
            let capture_observation = Arc::clone(&capture_observation);
            async move {
                let mut stream_results = Vec::with_capacity(stream_items.len());
                for (input_index, observation) in stream_items {
                    let context = crawler_operation_context(observation.command.listing_source_id);
                    let listing_source_id = observation.command.listing_source_id;
                    let outcome = match capture_observation.execute(&context, observation.command).await {
                        Ok(CaptureProductListingRawObservationResult::Changed { .. })
                        | Ok(CaptureProductListingRawObservationResult::Unchanged { .. })
                        | Ok(CaptureProductListingRawObservationResult::Duplicate { .. })
                        | Ok(CaptureProductListingRawObservationResult::Stale { .. }) => ProductListingRawCaptureOutcome::Persisted,
                        Err(product_listing_service::use_cases::CaptureProductListingRawObservationError::ListingSourceNotFound) => {
                            tracing::info!(
                                listing_source_id = %listing_source_id,
                                request_id = %context.request_id,
                                correlation_id = %context.correlation_id,
                                outcome = "discarded_missing_source",
                                "Discarded stale crawler raw observation for deleted ListingSource"
                            );
                            ProductListingRawCaptureOutcome::DiscardedMissingSource
                        }
                        Err(error) => {
                            warn!(
                                error = %error,
                                listing_source_id = %listing_source_id,
                                request_id = %context.request_id,
                                correlation_id = %context.correlation_id,
                                "Crawler raw ProductListing capture failed; observation remains retryable"
                            );
                            ProductListingRawCaptureOutcome::RetryableFailure
                        }
                    };
                    stream_results.push((input_index, outcome));
                }
                stream_results
            }
        }))
        .buffer_unordered(self.max_concurrent_captures)
        .collect::<Vec<_>>()
        .await;

        for stream_results in outcomes {
            for (input_index, outcome) in stream_results {
                results[input_index] = outcome;
            }
        }

        results
    }
}

fn crawler_operation_context(listing_source_id: ListingSourceId) -> OperationContext {
    let key = format!("crawler-raw-capture:{listing_source_id}");
    OperationContext {
        principal: Principal::Service("crawler".to_owned()),
        request_id: RequestId::new(key.clone()),
        correlation_id: CorrelationId::new(key),
    }
}

/// Writes display-only raw-capture snapshots. The file is never a replay source.
pub struct FileProductListingRawCaptureService {
    output_path: std::path::PathBuf,
}

impl FileProductListingRawCaptureService {
    pub fn new(output_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            output_path: output_path.into(),
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RawCaptureSnapshot {
    listing_source_id: String,
    ingestion_method: String,
    source_record_key: String,
    action: String,
    payload_format: String,
    payload_schema_version: u16,
    raw_values_schema_version: u16,
    source_payload: serde_json::Value,
    raw_values: serde_json::Value,
    normalization_context: serde_json::Value,
    provenance: serde_json::Value,
}

impl From<&ProductListingRawCaptureItem> for RawCaptureSnapshot {
    fn from(item: &ProductListingRawCaptureItem) -> Self {
        let command = &item.command;
        Self {
            listing_source_id: command.listing_source_id.to_string(),
            ingestion_method: command.ingestion_method.as_str().to_owned(),
            source_record_key: command.source_record_key.clone(),
            action: command.input.operation().as_str().to_owned(),
            payload_format: command.input.payload_format().as_str().to_owned(),
            payload_schema_version: command.input.payload_schema_version(),
            raw_values_schema_version: command.input.raw_values_schema_version(),
            source_payload: command.input.source_payload().value().clone(),
            raw_values: command.input.raw_values().value().clone(),
            normalization_context: command.input.normalization_context().value().clone(),
            provenance: command.provenance.value().clone(),
        }
    }
}

#[async_trait]
impl ProductListingRawCaptureService for FileProductListingRawCaptureService {
    async fn capture(
        &self,
        observations: Vec<ProductListingRawCaptureItem>,
    ) -> Vec<ProductListingRawCaptureOutcome> {
        if observations.is_empty() {
            return Vec::new();
        }

        let mut snapshots: Vec<serde_json::Value> = if self.output_path.exists() {
            match std::fs::read_to_string(&self.output_path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
            {
                Some(snapshots) => snapshots,
                None => {
                    return vec![
                        ProductListingRawCaptureOutcome::RetryableFailure;
                        observations.len()
                    ];
                }
            }
        } else {
            Vec::new()
        };
        let new_snapshots = match observations
            .iter()
            .map(RawCaptureSnapshot::from)
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(snapshots) => snapshots,
            Err(_) => {
                return vec![ProductListingRawCaptureOutcome::RetryableFailure; observations.len()];
            }
        };
        snapshots.extend(new_snapshots);
        let json = match serde_json::to_string_pretty(&snapshots) {
            Ok(json) => json,
            Err(_) => {
                return vec![ProductListingRawCaptureOutcome::RetryableFailure; observations.len()];
            }
        };
        match std::fs::write(&self.output_path, json) {
            Ok(()) => {
                debug!(count = observations.len(), path = %self.output_path.display(), "Wrote raw capture snapshots to file");
                vec![ProductListingRawCaptureOutcome::Persisted; observations.len()]
            }
            Err(error) => {
                warn!(error = %error, path = %self.output_path.display(), "Failed to write raw capture snapshots");
                vec![ProductListingRawCaptureOutcome::RetryableFailure; observations.len()]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_normalization::{
        NormalizationContext, RawProductListingOperation, RawProductListingPayloadFormat,
        RawProductListingValues, SourcePayload,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone, Copy, Default)]
    enum FakeCaptureOutcome {
        #[default]
        Unchanged,
        Duplicate,
        Stale,
    }

    #[derive(Default)]
    struct FakeCaptureUseCase {
        commands: Arc<Mutex<Vec<CaptureProductListingRawObservationCommand>>>,
        fail: bool,
        outcome: FakeCaptureOutcome,
    }

    #[async_trait]
    impl CaptureProductListingRawObservationUseCase for FakeCaptureUseCase {
        async fn execute(
            &self,
            _: &OperationContext,
            command: CaptureProductListingRawObservationCommand,
        ) -> Result<
            CaptureProductListingRawObservationResult,
            product_listing_service::use_cases::CaptureProductListingRawObservationError,
        > {
            self.commands
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(command.clone());
            if self.fail {
                return Err(product_listing_service::use_cases::CaptureProductListingRawObservationError::ListingSourceNotFound);
            }
            let product_listing_raw_stream_id =
                product_listing_service::ports::ProductListingRawStreamId::from_uuid(
                    uuid::Uuid::new_v4(),
                );
            Ok(match self.outcome {
                FakeCaptureOutcome::Unchanged => {
                    CaptureProductListingRawObservationResult::Unchanged {
                        product_listing_raw_stream_id,
                        latest_revision: 1,
                    }
                }
                FakeCaptureOutcome::Duplicate => {
                    CaptureProductListingRawObservationResult::Duplicate {
                        product_listing_raw_stream_id,
                        latest_revision: 1,
                    }
                }
                FakeCaptureOutcome::Stale => CaptureProductListingRawObservationResult::Stale {
                    product_listing_raw_stream_id,
                    latest_revision: 1,
                },
            })
        }
    }

    fn item(listing_source_id: ListingSourceId, url: &str) -> ProductListingRawCaptureItem {
        let url = Url::parse(url).unwrap_or_else(|error| panic!("test URL: {error}"));
        let input = ProductListingNormalizationInput::new(
            RawProductListingOperation::Upsert,
            RawProductListingPayloadFormat::CrawlerExtractedProduct,
            1,
            1,
            SourcePayload::new(serde_json::json!({"title": "Chair"}))
                .unwrap_or_else(|error| panic!("payload: {error}")),
            RawProductListingValues::new(serde_json::json!({}))
                .unwrap_or_else(|error| panic!("values: {error}")),
            NormalizationContext::new(serde_json::json!({}))
                .unwrap_or_else(|error| panic!("context: {error}")),
        )
        .unwrap_or_else(|error| panic!("input: {error}"));
        let provenance = RawProductListingProvenance::new(serde_json::json!({}))
            .unwrap_or_else(|error| panic!("provenance: {error}"));
        ProductListingRawCaptureItem::crawler(listing_source_id, &url, input, provenance)
    }

    #[tokio::test]
    async fn should_capture_each_raw_observation_without_merging_streams() {
        let listing_source_id = ListingSourceId::new();
        let use_case = Arc::new(FakeCaptureUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductListingRawCaptureServiceImpl::new(use_case, 2);

        let outcome = service
            .capture(vec![
                item(listing_source_id, "https://example.test/products/one"),
                item(listing_source_id, "https://example.test/products/two"),
            ])
            .await;

        assert_eq!(
            vec![
                ProductListingRawCaptureOutcome::Persisted,
                ProductListingRawCaptureOutcome::Persisted,
            ],
            outcome
        );
        let commands = commands
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(2, commands.len());
        assert_eq!("WEB_CRAWL", commands[0].ingestion_method.as_str());
        assert_eq!(
            "https://example.test/products/one",
            commands[0].source_record_key
        );
        assert_eq!(
            "https://example.test/products/two",
            commands[1].source_record_key
        );
        assert!(commands[0].provider_receipt.is_none());
        assert!(commands[1].provider_receipt.is_none());
    }

    #[tokio::test]
    async fn should_accept_duplicate_and_stale_capture_outcomes() {
        for outcome in [FakeCaptureOutcome::Duplicate, FakeCaptureOutcome::Stale] {
            let service = ProductListingRawCaptureServiceImpl::new(
                Arc::new(FakeCaptureUseCase {
                    outcome,
                    ..Default::default()
                }),
                1,
            );

            assert_eq!(
                vec![ProductListingRawCaptureOutcome::Persisted],
                service
                    .capture(vec![item(
                        ListingSourceId::new(),
                        "https://example.test/products/one"
                    )])
                    .await
            );
        }
    }

    #[tokio::test]
    async fn should_discard_capture_when_listing_source_is_missing() {
        let service = ProductListingRawCaptureServiceImpl::new(
            Arc::new(FakeCaptureUseCase {
                fail: true,
                ..Default::default()
            }),
            1,
        );
        assert_eq!(
            vec![ProductListingRawCaptureOutcome::DiscardedMissingSource],
            service
                .capture(vec![item(
                    ListingSourceId::new(),
                    "https://example.test/products/one"
                )])
                .await
        );
    }
}

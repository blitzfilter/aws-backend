mod support;

use test_api::{OpenSearch, Postgres, Sequin};

const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");
const OPENSEARCH: OpenSearch = OpenSearch();
const WORKER_SEQUIN: Sequin = Sequin::worker_webhook();
const WORKER_ACCEPTANCE: support::WorkerAcceptanceHarness = support::WorkerAcceptanceHarness::new();

#[path = "worker_cases/notification_delivery.rs"]
mod notification_delivery;
#[path = "worker_cases/product_content_assessment.rs"]
mod product_content_assessment;
#[path = "worker_cases/product_embedding.rs"]
mod product_embedding;
#[path = "worker_cases/product_listing_raw_normalization.rs"]
mod product_listing_raw_normalization;
#[path = "worker_cases/product_opensearch.rs"]
mod product_opensearch;
#[path = "worker_cases/product_translation.rs"]
mod product_translation;
#[path = "worker_cases/search_filter_percolator.rs"]
mod search_filter_percolator;
#[path = "worker_cases/search_filter_projection.rs"]
mod search_filter_projection;
#[path = "worker_cases/watchlist_notifications.rs"]
mod watchlist_notifications;

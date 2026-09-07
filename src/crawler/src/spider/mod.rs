pub mod advisory_lock;
pub mod candidate_service;
pub mod classification;
pub mod discovery;
pub mod service;
pub mod utils;

pub use classification::url_metadata::{CrawledUrlMetadata, CrawlerDisposition, UrlClass};
pub use service::SpiderRunResult;
pub use service::{SpiderService, SpiderServiceConfig, SpiderServiceError};

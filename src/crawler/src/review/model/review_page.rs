use crate::{CrawlerReviewId, CrawlerReviewPageId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReviewPage {
    pub review_page_id: CrawlerReviewPageId,
    pub review_id: CrawlerReviewId,
    pub url: String,
    pub role: String,
    pub html_hash: String,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched: OffsetDateTime,
}

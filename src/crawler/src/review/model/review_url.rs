use crate::{CrawlerReviewId, CrawlerReviewUrlId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerReviewUrl {
    pub review_url_id: CrawlerReviewUrlId,
    pub review_id: CrawlerReviewId,
    pub url: String,
    pub previous_class: Option<String>,
    pub current_pattern_match: Option<bool>,
    pub candidate_pattern_match: Option<bool>,
    pub candidate_class: Option<String>,
}

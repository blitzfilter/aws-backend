use super::SchemaCandidateEvaluation;
use crate::CrawlerReviewId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaMatrix {
    pub review_id: Option<CrawlerReviewId>,
    pub candidates: Vec<SchemaCandidateEvaluation>,
}

use crate::{CrawlerDomainId, CrawlerReviewId};
use listing_source_core::ListingSourceId;
use std::sync::Arc;

use crate::review::repository::CrawlerReviewRepository;
use crate::spider::classification::url_classification_service::{
    UrlClassificationError, UrlClassificationService,
};
use crate::spider::classification::url_pattern_repository::{
    ListingSourceUrlPatternRepository, UrlPatternState,
};

use regex::Regex;
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Error)]
pub enum UrlPatternServiceError {
    #[error(transparent)]
    Repository(#[from] sqlx::Error),

    #[error(transparent)]
    Regex(#[from] regex::Error),

    #[error(transparent)]
    Classification(#[from] UrlClassificationError),

    #[error(
        "URL pattern generation is blocked pending review '{review_id}' for ListingSource '{listing_source_id}'"
    )]
    PendingReview {
        listing_source_id: ListingSourceId,
        review_id: CrawlerReviewId,
    },
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait UrlPatternService: Send + Sync {
    /// Loads the persisted pattern for `listing_source_id` from the repository.
    ///
    /// Returns `None` when no pattern has been stored yet or when the stored
    /// value is `NULL` in the database.
    async fn load_pattern_for_domain(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<Option<Regex>, UrlPatternServiceError>;

    async fn save_pattern_for_domain(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: &Regex,
    ) -> Result<(), UrlPatternServiceError>;

    /// Clears a completed classification so a later crawl may infer again.
    async fn reset_pattern_classification(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), UrlPatternServiceError>;

    /// Asks the inference client to classify a product URL pattern from `urls`, persists the
    /// result when one is found, and returns it.
    ///
    /// This is the fallback used when no stored pattern exists or when
    /// the stored pattern must be refreshed after a failed crawl.
    async fn classify_and_save(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        urls: &[String],
    ) -> Result<Option<Regex>, UrlPatternServiceError>;

    /// Marks one configured crawler domain as crawled now.
    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), UrlPatternServiceError>;
}

pub struct UrlPatternServiceImpl {
    repository: Arc<dyn ListingSourceUrlPatternRepository>,
    classification_service: Box<dyn UrlClassificationService>,
    review_repository: Option<CrawlerReviewRepository>,
    review_required: bool,
}

impl UrlPatternServiceImpl {
    pub fn new(
        repository: Arc<dyn ListingSourceUrlPatternRepository>,
        classification_service: Box<dyn UrlClassificationService>,
    ) -> Self {
        Self {
            repository,
            classification_service,
            review_repository: None,
            review_required: false,
        }
    }

    pub fn new_with_review(
        repository: Arc<dyn ListingSourceUrlPatternRepository>,
        classification_service: Box<dyn UrlClassificationService>,
        review_repository: CrawlerReviewRepository,
        review_required: bool,
    ) -> Self {
        Self {
            repository,
            classification_service,
            review_repository: Some(review_repository),
            review_required,
        }
    }
}

#[async_trait::async_trait]
impl UrlPatternService for UrlPatternServiceImpl {
    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id))]
    async fn load_pattern_for_domain(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<Option<Regex>, UrlPatternServiceError> {
        let record = self
            .repository
            .find_pattern(listing_source_id, domain_id)
            .await?;

        let Some(record) = record else {
            return Ok(None);
        };

        let Some(raw_pattern) = record.url_pattern else {
            return Ok(None);
        };

        let pattern = Regex::new(&raw_pattern)?;
        Ok(Some(pattern))
    }

    async fn save_pattern_for_domain(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: &Regex,
    ) -> Result<(), UrlPatternServiceError> {
        self.repository
            .save_pattern(listing_source_id, domain_id, Some(pattern.as_str()))
            .await?;
        Ok(())
    }

    async fn reset_pattern_classification(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), UrlPatternServiceError> {
        self.repository
            .save_pattern(listing_source_id, domain_id, None)
            .await?;
        Ok(())
    }

    #[tracing::instrument(
        skip(self, urls),
        fields(listing_source_id = %listing_source_id, domain_id = %domain_id, crawl_root_url = %crawl_root_url, url_count = urls.len())
    )]
    async fn classify_and_save(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        crawl_root_url: &str,
        urls: &[String],
    ) -> Result<Option<Regex>, UrlPatternServiceError> {
        if self
            .repository
            .find_pattern(listing_source_id, domain_id)
            .await?
            .is_some_and(|record| record.url_pattern_state == UrlPatternState::NoPattern)
        {
            return Ok(None);
        }

        if self.review_required
            && let Some(review_repository) = &self.review_repository
            && let Some(review_id) = review_repository
                .latest_pending_url_pattern_review_id(listing_source_id, domain_id)
                .await?
        {
            return Err(UrlPatternServiceError::PendingReview {
                listing_source_id: *listing_source_id,
                review_id,
            });
        }

        self.repository
            .increment_listing_source_llm_calls(listing_source_id, 1)
            .await?;
        let pattern = self
            .classification_service
            .find_product_url_pattern(urls)
            .await?;

        if self.review_required
            && let Some(review_repository) = &self.review_repository
        {
            let current_pattern = self
                .load_pattern_for_domain(listing_source_id, domain_id)
                .await?;
            let review_id = review_repository
                .create_url_pattern_review(
                    listing_source_id,
                    domain_id,
                    "url_pattern_generation",
                    pattern.as_ref(),
                    urls,
                    current_pattern.as_ref(),
                )
                .await
                .map_err(|err| {
                    UrlPatternServiceError::Repository(sqlx::Error::Protocol(err.to_string()))
                })?;
            return Err(UrlPatternServiceError::PendingReview {
                listing_source_id: *listing_source_id,
                review_id,
            });
        }

        match pattern.as_ref() {
            Some(pattern) => {
                self.save_pattern_for_domain(listing_source_id, domain_id, pattern)
                    .await?;
                debug!(crawl_root_url, domain_id = %domain_id, "Persisted product URL pattern");
            }
            None => {
                self.repository
                    .save_no_pattern(listing_source_id, domain_id)
                    .await?;
                debug!(crawl_root_url, domain_id = %domain_id, "Persisted no product URL pattern");
            }
        }

        Ok(pattern)
    }

    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id, domain_id = %domain_id))]
    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), UrlPatternServiceError> {
        self.repository
            .mark_as_crawled(listing_source_id, domain_id)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spider::classification::url_classification_service::MockUrlClassificationService;
    use crate::spider::classification::url_pattern_repository::{
        ListingSourceUrlPatternRecord, MockListingSourceUrlPatternRepository,
    };
    use listing_source_core::Domain;

    #[tokio::test]
    async fn should_not_reclassify_completed_no_pattern_domain_until_reset() {
        let listing_source_id = ListingSourceId::new();
        let domain_id = CrawlerDomainId::new();
        let mut repository = MockListingSourceUrlPatternRepository::new();
        repository
            .expect_find_pattern()
            .times(1)
            .returning(move |_, _| {
                Box::pin(async move {
                    Ok(Some(ListingSourceUrlPatternRecord {
                        listing_source_id,
                        domain_id,
                        listing_source_domain: Domain::try_from("example.com").unwrap(),
                        url_pattern: None,
                        url_pattern_state: UrlPatternState::NoPattern,
                        last_crawled: None,
                    }))
                })
            });
        let service = UrlPatternServiceImpl::new(
            Arc::new(repository),
            Box::new(MockUrlClassificationService::new()),
        );

        let pattern = service
            .classify_and_save(
                &listing_source_id,
                &domain_id,
                "https://example.com",
                &["https://example.com/item/1".to_owned()],
            )
            .await
            .unwrap();

        assert!(pattern.is_none());
    }
}

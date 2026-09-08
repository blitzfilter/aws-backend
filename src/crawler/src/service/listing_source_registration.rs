//! Synchronizes crawler-local ListingSource crawl eligibility from business truth.

use async_trait::async_trait;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use money::Currency;
use sqlx::PgPool;
use tracing::{info, warn};

/// Crawler-safe business identity.
///
/// The business reader supplies canonical ListingSource values. `crawl_enabled` and
/// `fallback_currency` are crawler-local state derived from a complete successful
/// `WEB_CRAWL` source read.
#[derive(Debug, Clone)]
pub struct RegisteredListingSource {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: ListingSourceName,
    pub listing_source_slug: ListingSourceSlugId,
    pub crawl_enabled: bool,
    pub fallback_currency: Option<Currency>,
}

/// Result of one atomically applied business snapshot.
#[derive(Debug, Default)]
pub struct ListingSourceSnapshotResult {
    pub disabled: u64,
    pub enabled_without_domains: Vec<RegisteredListingSource>,
}

/// Reads the authoritative crawler scope.
#[async_trait]
#[mockall::automock]
pub trait ListingSourceRegistrationSource: Send + Sync {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError>;
}

/// Applies a complete authoritative ListingSource snapshot to crawler-local state.
#[async_trait]
#[mockall::automock]
pub trait ListingSourceRegistrationRepository: Send + Sync {
    async fn apply_snapshot(
        &self,
        listing_sources: &[RegisteredListingSource],
    ) -> Result<ListingSourceSnapshotResult, sqlx::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum ListingSourceSyncError {
    #[error("failed to read registered listing sources: {0}")]
    FetchError(String),
    #[error("crawler-local database error: {0}")]
    DatabaseError(#[from] sqlx::Error),
}

pub struct ListingSourceRegistrationService {
    source: Box<dyn ListingSourceRegistrationSource>,
    repository: Box<dyn ListingSourceRegistrationRepository>,
}

impl ListingSourceRegistrationService {
    pub fn new(
        source: Box<dyn ListingSourceRegistrationSource>,
        repository: Box<dyn ListingSourceRegistrationRepository>,
    ) -> Self {
        Self { source, repository }
    }

    #[tracing::instrument(name = "listing_source_registration_sync", skip(self))]
    pub async fn sync(&self) -> Result<usize, ListingSourceSyncError> {
        let listing_sources = self.source.fetch_registered_listing_sources().await?;
        let enabled_count = listing_sources
            .iter()
            .filter(|listing_source| listing_source.crawl_enabled)
            .count();

        let result = self.repository.apply_snapshot(&listing_sources).await?;
        if result.disabled > 0 {
            info!(
                disabled = result.disabled,
                "disabled crawler-local listing sources absent from business crawl scope"
            );
        }
        for listing_source in result.enabled_without_domains {
            warn!(
                event = "crawler.listing_source_unconfigured",
                listing_source_id = %listing_source.listing_source_id,
                listing_source_name = %listing_source.listing_source_name,
                "enabled ListingSource has no crawler-local domains"
            );
        }

        Ok(enabled_count)
    }
}

pub struct ListingSourceRegistrationRepositoryImpl {
    pool: PgPool,
}

impl ListingSourceRegistrationRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ListingSourceRegistrationRepository for ListingSourceRegistrationRepositoryImpl {
    async fn apply_snapshot(
        &self,
        listing_sources: &[RegisteredListingSource],
    ) -> Result<ListingSourceSnapshotResult, sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        let enabled_ids = listing_sources
            .iter()
            .filter(|listing_source| listing_source.crawl_enabled)
            .map(|listing_source| uuid::Uuid::from(listing_source.listing_source_id))
            .collect::<Vec<_>>();

        for listing_source in listing_sources {
            sqlx::query(
                "INSERT INTO listing_sources \
                    (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled, fallback_currency, created, updated) \
                 VALUES ($1, $2, $3, $4, $5, NOW(), NOW()) \
                 ON CONFLICT (listing_source_id) DO UPDATE SET \
                    listing_source_name = EXCLUDED.listing_source_name, \
                    listing_source_slug = EXCLUDED.listing_source_slug, \
                    crawl_enabled = EXCLUDED.crawl_enabled, \
                    fallback_currency = EXCLUDED.fallback_currency, \
                    updated = NOW()",
            )
            .bind(uuid::Uuid::from(listing_source.listing_source_id))
            .bind(listing_source.listing_source_name.as_ref())
            .bind(listing_source.listing_source_slug.to_string())
            .bind(listing_source.crawl_enabled)
            .bind(listing_source.fallback_currency.map(Currency::as_str))
            .execute(&mut *transaction)
            .await?;
        }

        let disabled = sqlx::query(
            "UPDATE listing_sources \
             SET crawl_enabled = FALSE, updated = NOW() \
             WHERE crawl_enabled = TRUE \
               AND NOT (listing_source_id = ANY($1::uuid[]))",
        )
        .bind(&enabled_ids)
        .execute(&mut *transaction)
        .await?
        .rows_affected();

        let unconfigured_ids = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT s.listing_source_id \
             FROM listing_sources s \
             WHERE s.crawl_enabled = TRUE \
               AND NOT EXISTS ( \
                   SELECT 1 FROM listing_source_domains sd \
                   WHERE sd.listing_source_id = s.listing_source_id \
               )",
        )
        .fetch_all(&mut *transaction)
        .await?;

        transaction.commit().await?;

        let enabled_without_domains = listing_sources
            .iter()
            .filter(|listing_source| {
                listing_source.crawl_enabled
                    && unconfigured_ids
                        .contains(&uuid::Uuid::from(listing_source.listing_source_id))
            })
            .cloned()
            .collect();

        Ok(ListingSourceSnapshotResult {
            disabled,
            enabled_without_domains,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing_source() -> RegisteredListingSource {
        RegisteredListingSource {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: ListingSourceName::try_from("Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            listing_source_slug: ListingSourceSlugId::raw("source")
                .unwrap_or_else(|error| panic!("valid test listing source slug: {error}")),
            crawl_enabled: true,
            fallback_currency: None,
        }
    }

    #[tokio::test]
    async fn should_apply_one_complete_snapshot_after_a_successful_business_read() {
        let source_listing_source = listing_source();
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(move || {
                let source = source_listing_source.clone();
                Box::pin(async move { Ok(vec![source]) })
            });

        let mut repository = MockListingSourceRegistrationRepository::new();
        repository
            .expect_apply_snapshot()
            .times(1)
            .withf(|sources| sources.len() == 1)
            .returning(|_| Box::pin(async { Ok(ListingSourceSnapshotResult::default()) }));

        let count = ListingSourceRegistrationService::new(Box::new(source), Box::new(repository))
            .sync()
            .await;
        assert!(matches!(count, Ok(1)));
    }

    #[tokio::test]
    async fn should_not_apply_snapshot_when_business_read_fails() {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .returning(|| {
                Box::pin(async {
                    Err(ListingSourceSyncError::FetchError("unavailable".to_owned()))
                })
            });

        let result = ListingSourceRegistrationService::new(
            Box::new(source),
            Box::new(MockListingSourceRegistrationRepository::new()),
        )
        .sync()
        .await;
        assert!(matches!(result, Err(ListingSourceSyncError::FetchError(_))));
    }
}

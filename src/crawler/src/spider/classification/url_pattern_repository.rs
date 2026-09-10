//! Repository for crawler-domain URL patterns and crawl scheduling state.

use crate::CrawlerDomainId;
use async_trait::async_trait;
use listing_source_core::{Domain, ListingSourceId};
use sqlx::{FromRow, PgPool, Row};
use strum::IntoEnumIterator;
use thiserror::Error;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum UrlPatternState {
    Unknown,
    Matched,
    NoPattern,
}

impl UrlPatternState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::Matched => "MATCHED",
            Self::NoPattern => "NO_PATTERN",
        }
    }
}

#[derive(Debug, Error)]
#[error("invalid persisted URL-pattern state: {0}")]
struct UrlPatternStateParseError(String);

impl UrlPatternState {
    fn from_persisted(value: String) -> Result<Self, UrlPatternStateParseError> {
        Self::iter()
            .find(|state| state.as_str() == value)
            .ok_or(UrlPatternStateParseError(value))
    }
}

#[derive(Debug, Clone)]
pub struct ListingSourceUrlPatternRecord {
    pub listing_source_id: ListingSourceId,
    pub domain_id: CrawlerDomainId,
    pub listing_source_domain: Domain,
    pub url_pattern: Option<String>,
    pub url_pattern_state: UrlPatternState,
    pub last_crawled: Option<OffsetDateTime>,
}

impl FromRow<'_, sqlx::postgres::PgRow> for ListingSourceUrlPatternRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let listing_source_id: uuid::Uuid = row.try_get("listing_source_id")?;
        let listing_source_domain =
            Domain::try_from(row.try_get::<String, _>("listing_source_domain")?)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        Ok(Self {
            listing_source_id: ListingSourceId::try_from(listing_source_id)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            domain_id: CrawlerDomainId::try_from(row.try_get::<uuid::Uuid, _>("domain_id")?)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            listing_source_domain,
            url_pattern: row.try_get("url_pattern")?,
            url_pattern_state: UrlPatternState::from_persisted(
                row.try_get::<String, _>("url_pattern_state")?,
            )
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
            last_crawled: row.try_get("last_crawled")?,
        })
    }
}

#[async_trait]
#[mockall::automock]
pub trait ListingSourceUrlPatternRepository: Send + Sync {
    async fn find_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<Option<ListingSourceUrlPatternRecord>, sqlx::Error>;

    async fn save_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: Option<&str>,
    ) -> Result<(), sqlx::Error>;

    async fn save_no_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), sqlx::Error>;

    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), sqlx::Error>;

    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error>;
}

pub struct ListingSourceUrlPatternRepositoryImpl {
    pool: PgPool,
}

impl ListingSourceUrlPatternRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ListingSourceUrlPatternRepository for ListingSourceUrlPatternRepositoryImpl {
    async fn find_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<Option<ListingSourceUrlPatternRecord>, sqlx::Error> {
        sqlx::query_as::<_, ListingSourceUrlPatternRecord>(
            "SELECT listing_source_id, domain_id, listing_source_domain, \
                    url_pattern, url_pattern_state, last_crawled \
             FROM listing_source_domains \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
    }

    async fn save_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        pattern: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_domains \
             SET url_pattern = $3, \
                 url_pattern_state = CASE WHEN $3 IS NULL THEN 'UNKNOWN' ELSE 'MATCHED' END \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .bind(pattern)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    async fn save_no_pattern(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_domains \
             SET url_pattern = NULL, url_pattern_state = 'NO_PATTERN' \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    async fn mark_as_crawled(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_domains \
             SET last_crawled = NOW() \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(domain_id.as_uuid())
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources \
             SET llm_calls_count = llm_calls_count + $2, updated = NOW() \
             WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(delta)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }
}

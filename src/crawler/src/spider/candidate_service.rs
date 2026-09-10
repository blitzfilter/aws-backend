use crate::CrawlerDomainId;
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::PgPool;

pub struct SpiderCandidate {
    pub listing_source_id: ListingSourceId,
    pub domain_id: CrawlerDomainId,
    pub listing_source_domain: String,
    pub crawl_failure_count: i32,
    pub last_crawl_error_kind: Option<String>,
}

#[async_trait]
#[mockall::automock]
pub trait SpiderCandidateService: Send + Sync {
    async fn get_candidates(
        &self,
        limit: i64,
        excluded_domain_ids: &[CrawlerDomainId],
    ) -> Result<Vec<SpiderCandidate>, sqlx::Error>;
    async fn mark_crawl_failure(
        &self,
        domain_id: &CrawlerDomainId,
        error_kind: &str,
        crawl_failure_count: i32,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<(), sqlx::Error>;
    async fn reset_crawl_failure(&self, domain_id: &CrawlerDomainId) -> Result<(), sqlx::Error>;
}

pub struct SpiderCandidateServiceImpl {
    pool: PgPool,
}

impl SpiderCandidateServiceImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct SpiderCandidateRow {
    listing_source_id: uuid::Uuid,
    domain_id: uuid::Uuid,
    listing_source_domain: String,
    crawl_failure_count: i32,
    last_crawl_error_kind: Option<String>,
}

#[async_trait]
impl SpiderCandidateService for SpiderCandidateServiceImpl {
    async fn get_candidates(
        &self,
        limit: i64,
        excluded_domain_ids: &[CrawlerDomainId],
    ) -> Result<Vec<SpiderCandidate>, sqlx::Error> {
        let excluded_domain_ids: Vec<uuid::Uuid> = excluded_domain_ids
            .iter()
            .copied()
            .map(CrawlerDomainId::into_uuid)
            .collect();
        let rows = sqlx::query_as::<_, SpiderCandidateRow>(
            r#"
            SELECT s.listing_source_id,
                   sd.domain_id,
                   sd.crawl_root_host AS listing_source_domain,
                   sd.crawl_failure_count,
                   sd.last_crawl_error_kind
            FROM listing_sources s
            JOIN listing_source_domains sd ON sd.listing_source_id = s.listing_source_id
            WHERE s.crawl_enabled = TRUE
              AND (sd.last_crawled IS NULL OR sd.last_crawled < NOW() - INTERVAL '7 days')
              AND (sd.next_crawl_at IS NULL OR sd.next_crawl_at <= NOW())
              AND NOT (sd.domain_id = ANY($2))
            ORDER BY sd.last_crawled NULLS FIRST
            LIMIT $1
            "#,
        )
        .bind(limit)
        .bind(excluded_domain_ids)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(SpiderCandidate {
                    listing_source_id: ListingSourceId::try_from(row.listing_source_id)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                    domain_id: CrawlerDomainId::try_from(row.domain_id)
                        .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                    listing_source_domain: row.listing_source_domain,
                    crawl_failure_count: row.crawl_failure_count,
                    last_crawl_error_kind: row.last_crawl_error_kind,
                })
            })
            .collect()
    }

    async fn mark_crawl_failure(
        &self,
        domain_id: &CrawlerDomainId,
        error_kind: &str,
        crawl_failure_count: i32,
        next_crawl_at: time::OffsetDateTime,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE listing_source_domains
             SET crawl_failure_count = $3,
                 last_crawl_error_kind = $2,
                 next_crawl_at = $4
             WHERE domain_id = $1",
        )
        .bind(domain_id.as_uuid())
        .bind(error_kind)
        .bind(crawl_failure_count)
        .bind(next_crawl_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn reset_crawl_failure(&self, domain_id: &CrawlerDomainId) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE listing_source_domains
             SET crawl_failure_count = 0,
                 last_crawl_error_kind = NULL,
                 next_crawl_at = NULL
             WHERE domain_id = $1",
        )
        .bind(domain_id.as_uuid())
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

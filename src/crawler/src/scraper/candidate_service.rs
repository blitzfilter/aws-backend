//! Service for fetching scraper candidates — product URLs that are due for re-scraping.
//!
//! A scraper candidate is a URL stored in `listing_source_urls` that is due for scraping by recency,
//! retry, and crawler disposition. Both active and sold URLs remain eligible so crawler evidence can
//! observe a later removal or restock. Local fingerprints describe the last local completion, not
//! current business custody. Only authoritative raw capture can confirm unchanged input.

use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use money::Currency;
use sqlx::PgPool;
use time::OffsetDateTime;
use url::Url;

use crate::scraper::scraper_service::DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE;
use crate::spider::classification::url_metadata::{
    CrawlerDisposition, CrawlerUrlWriteOutcome, UrlClass,
};

// ---------------------------------------------------------------------------
// ScraperCandidate
// ---------------------------------------------------------------------------

/// A product URL eligible for scraping with only crawler-operational state.
pub struct ScraperCandidate {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    pub fallback_currency: Option<Currency>,
    pub url_pattern: Option<String>,
    pub url: Url,
    pub last_scraped_hash: Option<String>,
    pub last_scraped_schema_fingerprint: Option<String>,
    pub last_captured_raw_input_sha256: Option<Vec<u8>>,
}

/// Per-ListingSource LLM usage snapshot for operational logging.
pub struct ListingSourceLlmUsage {
    pub listing_source_id: ListingSourceId,
    pub listing_source_name: String,
    pub llm_calls_count: i64,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

#[async_trait]
#[mockall::automock]
pub trait ScraperCandidateService: Send + Sync {
    async fn get_candidates(
        &self,
        domain_limit: i64,
        urls_per_domain: i64,
        excluded_domains: &[String],
    ) -> Result<Vec<ScraperCandidate>, sqlx::Error>;
    /// Returns a random sample of product URLs for a ListingSource (excluding the current
    /// URL) to seed first-time schema generation with additional page layouts.
    ///
    /// This query intentionally uses `ORDER BY RANDOM()` because the path is
    /// only used on schema cache misses, which are rare (typically one-time per
    /// ListingSource unless schema rows are reset).
    async fn get_random_product_urls_for_schema_seed(
        &self,
        listing_source_id: &ListingSourceId,
        exclude_url: &Url,
        limit: i64,
    ) -> Result<Vec<Url>, sqlx::Error>;
    #[allow(clippy::too_many_arguments)]
    async fn mark_as_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        hash: &str,
        schema_fingerprint: &str,
        raw_input_sha256: &[u8],
        disposition: CrawlerDisposition,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    /// Records a durable crawler removal capture and makes the URL eligible for later rechecks.
    async fn mark_removed(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        raw_input_sha256: &[u8],
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    /// Refresh local metadata only after independently confirming durable capture.
    /// Local page/schema/raw-input equality alone is not sufficient proof.
    async fn touch_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        hash: &str,
        schema_fingerprint: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    async fn set_disposition(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        disposition: CrawlerDisposition,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    async fn set_class(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        url_class: UrlClass,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
    #[allow(clippy::too_many_arguments)]
    async fn mark_fetch_failure(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;

    /// Record a non-HTTP scraper failure (schema error, normalization error, etc.).
    ///
    /// Unlike [`mark_fetch_failure`] this does **not** increment `failure_count` or
    /// set a `next_retry_at` backoff — these errors are not caused by the remote
    /// server being unavailable and should not suppress future fetches.
    async fn mark_scraper_failure(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;

    /// Increment per-ListingSource LLM call counter used by schema generation flows.
    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error>;

    /// Try to increment per-ListingSource LLM call counter if the configured max would
    /// not be exceeded. Returns `true` when incremented, `false` when blocked
    /// by the limit.
    async fn try_increment_listing_source_llm_calls_with_limit(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error>;

    /// Returns whether the per-ListingSource LLM-call budget is already exhausted.
    async fn is_listing_source_llm_budget_exhausted(
        &self,
        listing_source_id: &ListingSourceId,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error>;

    /// Returns per-ListingSource LLM call counts for the provided ListingSource IDs.
    async fn get_listing_source_llm_usage(
        &self,
        listing_source_ids: Vec<ListingSourceId>,
    ) -> Result<Vec<ListingSourceLlmUsage>, sqlx::Error>;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

pub struct ScraperCandidateServiceImpl {
    pool: PgPool,
    max_llm_calls_per_listing_source: i64,
}

impl ScraperCandidateServiceImpl {
    pub fn new(pool: PgPool) -> Self {
        Self::new_with_max_llm_calls_per_listing_source(
            pool,
            DEFAULT_MAX_LLM_CALLS_PER_LISTING_SOURCE,
        )
    }

    pub fn new_with_max_llm_calls_per_listing_source(
        pool: PgPool,
        max_llm_calls_per_listing_source: i64,
    ) -> Self {
        Self {
            pool,
            max_llm_calls_per_listing_source,
        }
    }
}

#[derive(sqlx::FromRow)]
struct ScraperCandidateRow {
    listing_source_id: uuid::Uuid,
    listing_source_name: String,
    fallback_currency: Option<String>,

    url_pattern: Option<String>,
    url: String,
    last_scraped_hash: Option<String>,
    last_scraped_schema_fingerprint: Option<String>,
    last_captured_raw_input_sha256: Option<Vec<u8>>,
}

const SCRAPER_CANDIDATE_QUERY: &str = r#"
    WITH eligible_urls AS (
        SELECT
            su.listing_source_id, s.listing_source_name, s.fallback_currency, sd.url_pattern, su.url,
            lower(substring(su.url from '^[a-z][a-z0-9+.-]*://([^/:?#]+)')) AS normalized_host,
            su.last_scraped,
            su.last_scraped_hash,
            su.last_scraped_schema_fingerprint,
            su.last_captured_raw_input_sha256
        FROM listing_source_urls su
        JOIN listing_sources s ON s.listing_source_id = su.listing_source_id
        JOIN listing_source_domains sd
          ON sd.listing_source_id = su.listing_source_id AND sd.domain_id = su.domain_id
        WHERE s.crawl_enabled = TRUE
          AND s.llm_calls_count < $3
          AND su.url_class = 'product'
          AND su.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
          AND (su.next_retry_at IS NULL OR su.next_retry_at <= NOW())
          AND (su.last_scraped IS NULL OR su.last_scraped < NOW() - INTERVAL '1 day')
          AND NOT EXISTS (
              SELECT 1
              FROM crawler_reviews cr
              WHERE cr.listing_source_id = su.listing_source_id
                AND cr.artifact_type = 'PRODUCT_SCHEMA'
                AND cr.status = 'PENDING_REVIEW'
          )
          AND NOT (
              lower(substring(su.url from '^[a-z][a-z0-9+.-]*://([^/:?#]+)')) = ANY($4)
          )
    ),
    selected_domains AS (
        SELECT normalized_host
        FROM eligible_urls
        WHERE normalized_host IS NOT NULL
        GROUP BY normalized_host
        ORDER BY random()
        LIMIT $1
    ),
    ranked_urls AS (
        SELECT
            eu.*,
            row_number() OVER (
                PARTITION BY eu.normalized_host
                ORDER BY eu.last_scraped NULLS FIRST, eu.url
            ) AS domain_url_rank
        FROM eligible_urls eu
        JOIN selected_domains sd ON sd.normalized_host = eu.normalized_host
    )
    SELECT
        listing_source_id, listing_source_name, fallback_currency, url_pattern, url,
        last_scraped_hash,
        last_scraped_schema_fingerprint,
        last_captured_raw_input_sha256
    FROM ranked_urls
    WHERE domain_url_rank <= $2
    ORDER BY normalized_host, domain_url_rank, url
    "#;

#[async_trait]
impl ScraperCandidateService for ScraperCandidateServiceImpl {
    async fn get_candidates(
        &self,
        domain_limit: i64,
        urls_per_domain: i64,
        excluded_domains: &[String],
    ) -> Result<Vec<ScraperCandidate>, sqlx::Error> {
        let rows = sqlx::query_as::<_, ScraperCandidateRow>(SCRAPER_CANDIDATE_QUERY)
            .bind(domain_limit)
            .bind(urls_per_domain)
            .bind(self.max_llm_calls_per_listing_source)
            .bind(excluded_domains)
            .fetch_all(&self.pool)
            .await?;

        let mut candidates = Vec::new();
        for row in rows {
            let Some(url) = Url::parse(&row.url).ok() else {
                continue;
            };
            let fallback_currency = row
                .fallback_currency
                .as_deref()
                .map(|value| {
                    Currency::from_code(value).ok_or_else(|| {
                        sqlx::Error::Decode(Box::new(std::io::Error::other(
                            "persisted crawler fallback currency is invalid",
                        )))
                    })
                })
                .transpose()?;
            candidates.push(ScraperCandidate {
                listing_source_id: ListingSourceId::try_from(row.listing_source_id)
                    .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
                listing_source_name: row.listing_source_name,
                fallback_currency,
                url_pattern: row.url_pattern,
                url,
                last_scraped_hash: row.last_scraped_hash,
                last_scraped_schema_fingerprint: row.last_scraped_schema_fingerprint,
                last_captured_raw_input_sha256: row.last_captured_raw_input_sha256,
            });
        }

        Ok(candidates)
    }

    async fn get_random_product_urls_for_schema_seed(
        &self,
        listing_source_id: &ListingSourceId,
        exclude_url: &Url,
        limit: i64,
    ) -> Result<Vec<Url>, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT su.url
            FROM listing_source_urls su
            JOIN listing_sources s ON s.listing_source_id = su.listing_source_id
            WHERE s.crawl_enabled = TRUE
              AND su.listing_source_id = $1
              AND su.url_class = 'product'
              AND su.crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
              AND su.url <> $2
            -- Intentional: schema seeding runs on a rare path (typically once per
            -- ListingSource), so ORDER BY RANDOM() keeps this simple. If rows per ListingSource grow
            -- to millions, switch to TABLESAMPLE BERNOULLI or keyset-random.
            ORDER BY RANDOM()
            LIMIT $3
            "#,
        )
        .bind(listing_source_id_uuid)
        .bind(exclude_url.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(raw_url,)| Url::parse(&raw_url).ok())
            .collect())
    }

    async fn mark_as_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        hash: &str,
        schema_fingerprint: &str,
        raw_input_sha256: &[u8],
        disposition: CrawlerDisposition,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_scraped = NOW(),
                 last_scraped_hash = $3,
                 last_scraped_schema_fingerprint = $4,
                 last_captured_raw_input_sha256 = $5,
                 crawler_disposition = $6,
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_error_message = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $7::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(hash)
        .bind(schema_fingerprint)
        .bind(raw_input_sha256)
        .bind(disposition.as_str())
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn mark_removed(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        raw_input_sha256: &[u8],
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_scraped = NOW(),
                 last_scraped_hash = NULL,
                 last_scraped_schema_fingerprint = NULL,
                 last_captured_raw_input_sha256 = $3,
                 crawler_disposition = 'ACTIVE',
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_error_message = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $4::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url.to_string())
        .bind(raw_input_sha256)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn touch_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        hash: &str,
        schema_fingerprint: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_scraped = NOW(),
                 last_scraped_hash = $3,
                 last_scraped_schema_fingerprint = $4,
                 failure_count = 0,
                 last_error_kind = NULL,
                 last_error_message = NULL,
                 last_status_code = NULL,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $5::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(hash)
        .bind(schema_fingerprint)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn set_disposition(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        disposition: CrawlerDisposition,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();
        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET crawler_disposition = $3,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $4::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(disposition.as_str())
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn set_class(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        url_class: UrlClass,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();
        let url_class_str = url_class.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET url_class = $3,
                 next_retry_at = NULL,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $4::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(url_class_str)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn mark_fetch_failure(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        status_code: Option<i32>,
        next_retry_at: OffsetDateTime,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET failure_count = failure_count + 1,
                 last_error_kind = $3,
                 last_error_message = $4,
                 last_status_code = $5,
                 next_retry_at = $6,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $7::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(error_kind)
        .bind(error_message)
        .bind(status_code)
        .bind(next_retry_at)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn mark_scraper_failure(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        error_kind: &str,
        error_message: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let listing_source_id_uuid: uuid::Uuid = (*listing_source_id).into();
        let url_str = url.to_string();

        let result = sqlx::query(
            "UPDATE listing_source_urls
             SET last_error_kind = $3,
                 last_error_message = $4,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND url = $2
               AND url_class = 'product'
               AND crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')
               AND last_captured_raw_input_sha256 IS NOT DISTINCT FROM $5::bytea",
        )
        .bind(listing_source_id_uuid)
        .bind(url_str)
        .bind(error_kind)
        .bind(error_message)
        .bind(expected_last_captured_raw_input_sha256)
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }

    async fn increment_listing_source_llm_calls(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE listing_sources
             SET llm_calls_count = llm_calls_count + $2,
                 updated = NOW()
             WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(delta)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn try_increment_listing_source_llm_calls_with_limit(
        &self,
        listing_source_id: &ListingSourceId,
        delta: i64,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_sources
             SET llm_calls_count = llm_calls_count + $2,
                 updated = NOW()
             WHERE listing_source_id = $1
               AND llm_calls_count + $2 <= $3",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(delta)
        .bind(max_calls)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn is_listing_source_llm_budget_exhausted(
        &self,
        listing_source_id: &ListingSourceId,
        max_calls: i64,
    ) -> Result<bool, sqlx::Error> {
        let exhausted = sqlx::query_scalar::<_, bool>(
            "SELECT llm_calls_count >= $2
             FROM listing_sources
             WHERE listing_source_id = $1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(max_calls)
        .fetch_optional(&self.pool)
        .await?
        .unwrap_or(false);

        Ok(exhausted)
    }

    async fn get_listing_source_llm_usage(
        &self,
        listing_source_ids: Vec<ListingSourceId>,
    ) -> Result<Vec<ListingSourceLlmUsage>, sqlx::Error> {
        if listing_source_ids.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<uuid::Uuid> = listing_source_ids
            .into_iter()
            .map(uuid::Uuid::from)
            .collect();
        let rows: Vec<(uuid::Uuid, Option<String>, i64)> = sqlx::query_as(
            "SELECT listing_source_id, listing_source_name, llm_calls_count
             FROM listing_sources
             WHERE listing_source_id = ANY($1::uuid[])",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(id, name, llm_calls_count)| {
                let listing_source_id = ListingSourceId::try_from(id).map_err(|source| {
                    sqlx::Error::Decode(Box::new(PersistedListingSourceIdError {
                        value: id,
                        source,
                    }))
                })?;
                Ok(ListingSourceLlmUsage {
                    listing_source_id,
                    listing_source_name: name.unwrap_or_else(|| listing_source_id.to_string()),
                    llm_calls_count,
                })
            })
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid persisted ListingSourceId UUID `{value}`")]
struct PersistedListingSourceIdError {
    value: uuid::Uuid,
    #[source]
    source: domain_primitives::object_id::ObjectIdError,
}

#[cfg(test)]
mod candidate_query_tests {
    use super::*;
    use test_api::IntegrationTestService;

    const POSTGRES: test_api::Postgres = test_api::Postgres::new("src/crawler/migrations");

    async fn seed_product(pool: &PgPool) -> (ListingSourceId, Url) {
        let id = ListingSourceId::new();
        let domain_id = crate::CrawlerDomainId::new();
        let url = Url::parse("https://custody.example.test/products/one").unwrap();
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_name, listing_source_slug, crawl_enabled) VALUES ($1, 'Custody fixture', 'custody-fixture', TRUE)")
            .bind(id.as_uuid()).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO listing_source_domains (domain_id, listing_source_id, listing_source_domain, crawl_root_host) VALUES ($1, $2, 'custody.example.test', 'custody.example.test')")
            .bind(domain_id.as_uuid()).bind(id.as_uuid()).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO listing_source_urls (listing_source_id, domain_id, url, url_class, last_scraped, last_scraped_hash, last_scraped_schema_fingerprint, last_captured_raw_input_sha256) VALUES ($1, $2, $3, 'product', NOW() - INTERVAL '2 days', 'same-page', 'same-schema', $4)")
            .bind(id.as_uuid()).bind(domain_id.as_uuid()).bind(url.as_str()).bind(vec![3_u8; 32]).execute(pool).await.unwrap();
        (id, url)
    }

    #[serial_test::serial]
    #[test_api::aura_integration_test(services = [POSTGRES])]
    async fn should_clear_product_fingerprints_on_removal_and_fence_stale_removal_after_restore() {
        let pool = test_api::get_postgres_client().await;
        let (id, url) = seed_product(&pool).await;
        let service = ScraperCandidateServiceImpl::new(pool.clone());
        let removal = crate::scraper::raw_input::crawler_verified_removal_input(&url)
            .unwrap()
            .hash()
            .unwrap();
        assert_eq!(
            service
                .mark_removed(&id, &url, removal.as_bytes(), Some(&[3; 32]))
                .await
                .unwrap(),
            CrawlerUrlWriteOutcome::Applied
        );
        let (page, schema, hash, disposition, scraped): (Option<String>, Option<String>, Vec<u8>, String, bool) = sqlx::query_as("SELECT last_scraped_hash, last_scraped_schema_fingerprint, last_captured_raw_input_sha256, crawler_disposition, last_scraped IS NOT NULL FROM listing_source_urls WHERE url = $1")
            .bind(url.as_str()).fetch_one(&pool).await.unwrap();
        assert_eq!((page, schema), (None, None));
        assert_eq!(hash, removal.as_bytes());
        assert_eq!(disposition, "ACTIVE");
        assert!(scraped);

        sqlx::query("UPDATE listing_source_urls SET last_scraped = NOW() - INTERVAL '2 days' WHERE url = $1").bind(url.as_str()).execute(&pool).await.unwrap();
        let candidates = service.get_candidates(1, 1, &[]).await.unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "removed URL stays eligible for later recheck"
        );
        assert!(candidates[0].last_scraped_hash.is_none());
        assert!(candidates[0].last_scraped_schema_fingerprint.is_none());
        assert_eq!(
            service
                .mark_as_scraped(
                    &id,
                    &url,
                    "same-page",
                    "same-schema",
                    &[3; 32],
                    CrawlerDisposition::Active,
                    Some(removal.as_bytes())
                )
                .await
                .unwrap(),
            CrawlerUrlWriteOutcome::Applied
        );
        assert_eq!(
            service
                .mark_removed(&id, &url, removal.as_bytes(), Some(removal.as_bytes()))
                .await
                .unwrap(),
            CrawlerUrlWriteOutcome::NoopStale
        );
        let (page, schema, hash): (String, String, Vec<u8>) = sqlx::query_as("SELECT last_scraped_hash, last_scraped_schema_fingerprint, last_captured_raw_input_sha256 FROM listing_source_urls WHERE url = $1")
            .bind(url.as_str()).fetch_one(&pool).await.unwrap();
        assert_eq!(page, "same-page");
        assert_eq!(schema, "same-schema");
        assert_eq!(hash, vec![3; 32]);
    }

    #[serial_test::serial]
    #[test_api::aura_integration_test(services = [POSTGRES])]
    async fn should_leave_due_progress_unchanged_when_local_mark_statement_fails() {
        let pool = test_api::get_postgres_client().await;
        let (id, url) = seed_product(&pool).await;
        let service = ScraperCandidateServiceImpl::new(pool.clone());
        // Existing byte-length constraint injects a statement failure; no DDL or trigger.
        assert!(
            service
                .mark_as_scraped(
                    &id,
                    &url,
                    "new-page",
                    "new-schema",
                    &[9; 31],
                    CrawlerDisposition::DormantSold,
                    Some(&[3; 32])
                )
                .await
                .is_err()
        );
        let candidates = service.get_candidates(1, 1, &[]).await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].last_scraped_hash.as_deref(),
            Some("same-page")
        );
        assert_eq!(
            candidates[0].last_scraped_schema_fingerprint.as_deref(),
            Some("same-schema")
        );
        assert_eq!(
            candidates[0].last_captured_raw_input_sha256.as_deref(),
            Some(&[3; 32][..])
        );
        assert_eq!(
            service
                .mark_as_scraped(
                    &id,
                    &url,
                    "new-page",
                    "new-schema",
                    &[9; 32],
                    CrawlerDisposition::DormantSold,
                    Some(&[3; 32])
                )
                .await
                .unwrap(),
            CrawlerUrlWriteOutcome::Applied
        );
        assert!(service.get_candidates(1, 1, &[]).await.unwrap().is_empty());
    }

    #[test]
    fn should_select_active_and_sold_urls_for_scraping() {
        assert!(
            SCRAPER_CANDIDATE_QUERY.contains("crawler_disposition IN ('ACTIVE', 'DORMANT_SOLD')")
        );
    }
}

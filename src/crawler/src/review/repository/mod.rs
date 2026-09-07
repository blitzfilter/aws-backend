use crate::CrawlerDomainId;
use crate::review::model::*;
use crate::review::schema_evaluation::evaluate_schema_matrix_for_live_review_pages;
use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::product_schema_repository::{
    ListingSourceProductSchemaRepository, ListingSourceProductSchemaRepositoryImpl,
};
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::spider::utils::url::CrawledUrl;
use listing_source_core::ListingSourceId;
use regex::Regex;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use tracing::info;
use url::Url;

mod schema_payload;

use schema_payload::{
    approval_product_schemas, parse_schemas_payload, update_schema_field_payload,
    validate_product_schemas,
};

#[derive(Debug, thiserror::Error)]
pub enum ReviewRepositoryError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Regex(#[from] regex::Error),
    #[error("review not found: {0}")]
    NotFound(uuid::Uuid),
    #[error("review {0} is not pending review")]
    NotPending(uuid::Uuid),
    #[error("review {0} has unsupported artifact type {1}")]
    UnsupportedArtifact(uuid::Uuid, String),
    #[error("invalid schema field `{0}`")]
    InvalidSchemaField(String),
    #[error("field `{0}` is required and cannot be deleted")]
    RequiredSchemaField(String),
    #[error("invalid URL-pattern review candidate")]
    InvalidUrlPatternCandidate,
    #[error("invalid ProductSchema review candidate")]
    InvalidProductSchemaCandidate,
    #[error("review candidate changed during schema-matrix evaluation")]
    CandidateChangedDuringEvaluation,
}

#[derive(Clone)]
pub struct CrawlerReviewRepository {
    pool: PgPool,
}

pub struct EvaluatedSchemaMatrix {
    pub matrix: SchemaMatrix,
    candidate_version: i64,
    candidate_hash: String,
}

pub struct SchemaReviewWithStatusInput<'a> {
    pub listing_source_id: &'a ListingSourceId,
    pub reason: &'a str,
    pub schemas: &'a [ProductCssSelectorSchema],
    pub pages: Vec<SchemaReviewPageInput>,
    pub validation_summary: serde_json::Value,
    pub status: &'a str,
    pub notes: Option<&'a str>,
}

impl CrawlerReviewRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn has_pending_review(
        &self,
        listing_source_id: &ListingSourceId,
        artifact_type: &str,
    ) -> Result<bool, sqlx::Error> {
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM crawler_reviews
                WHERE listing_source_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW'
            )",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(artifact_type)
        .fetch_one(&self.pool)
        .await?;
        Ok(exists)
    }

    pub async fn latest_pending_review_id(
        &self,
        listing_source_id: &ListingSourceId,
        artifact_type: &str,
    ) -> Result<Option<uuid::Uuid>, sqlx::Error> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT review_id FROM crawler_reviews \
             WHERE listing_source_id = $1 AND artifact_type = $2 AND status = 'PENDING_REVIEW' \
             ORDER BY created DESC LIMIT 1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(artifact_type)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn has_pending_url_pattern_review(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM crawler_reviews \
             WHERE listing_source_id = $1 AND domain_id = $2 \
               AND artifact_type = 'URL_PATTERN' AND status = 'PENDING_REVIEW')",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(uuid::Uuid::from(*domain_id))
        .fetch_one(&self.pool)
        .await
    }

    pub async fn latest_pending_url_pattern_review_id(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
    ) -> Result<Option<uuid::Uuid>, sqlx::Error> {
        sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT review_id FROM crawler_reviews \
             WHERE listing_source_id = $1 AND domain_id = $2 \
               AND artifact_type = 'URL_PATTERN' AND status = 'PENDING_REVIEW' \
             ORDER BY created DESC LIMIT 1",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(uuid::Uuid::from(*domain_id))
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn list_reviews(&self, limit: i64) -> Result<Vec<CrawlerReview>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT r.review_id, r.listing_source_id, s.listing_source_name, r.domain_id, r.artifact_type, r.status, r.reason,
                    r.candidate_payload, r.validation_summary, r.reviewer_notes, r.created, r.updated, r.reviewed
             FROM crawler_reviews r
             LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
             ORDER BY
               CASE WHEN r.status = 'PENDING_REVIEW' THEN 0 ELSE 1 END,
               r.created DESC
             LIMIT $1",
        )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        rows.into_iter().map(row_to_review).collect()
    }

    pub async fn list_listing_sources(
        &self,
        limit: i64,
    ) -> Result<Vec<ListingSourceOverview>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT
                s.listing_source_id,
                s.listing_source_name,
                s.crawl_enabled,
                s.llm_calls_count,
                COALESCE(d.domain_count, 0) AS domain_count,
                d.last_successful_crawl,
                COALESCE(r.pending_reviews, 0) AS pending_reviews,
                COALESCE(u.product_urls, 0) AS product_urls,
                COALESCE(u.blocked_urls, 0) AS blocked_urls
             FROM listing_sources s
             LEFT JOIN (
                SELECT listing_source_id, COUNT(*) AS domain_count, MAX(last_crawled) AS last_successful_crawl
                FROM listing_source_domains GROUP BY listing_source_id
             ) d ON d.listing_source_id = s.listing_source_id
             LEFT JOIN (
                SELECT listing_source_id, COUNT(*) AS pending_reviews
                FROM crawler_reviews
                WHERE status = 'PENDING_REVIEW'
                GROUP BY listing_source_id
             ) r ON r.listing_source_id = s.listing_source_id
             LEFT JOIN (
                SELECT listing_source_id,
                       COUNT(*) FILTER (WHERE url_class = 'product') AS product_urls,
                       COUNT(*) FILTER (WHERE last_error_kind IN ('PendingSchemaReview', 'PendingUrlPatternReview')) AS blocked_urls
                FROM listing_source_urls
                GROUP BY listing_source_id
             ) u ON u.listing_source_id = s.listing_source_id
             ORDER BY s.crawl_enabled DESC, pending_reviews DESC, s.updated DESC
             LIMIT $1",
        )
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;

        let mut listing_sources = Vec::with_capacity(rows.len());
        for row in rows {
            listing_sources.push(ListingSourceOverview {
                listing_source_id: ListingSourceId::from(
                    row.try_get::<uuid::Uuid, _>("listing_source_id")?,
                ),
                listing_source_name: row.try_get("listing_source_name")?,
                crawl_enabled: row.try_get("crawl_enabled")?,
                llm_calls_count: row.try_get("llm_calls_count")?,
                domain_count: row.try_get("domain_count")?,
                last_successful_crawl: row.try_get("last_successful_crawl")?,
                pending_reviews: row.try_get("pending_reviews")?,
                product_urls: row.try_get("product_urls")?,
                blocked_urls: row.try_get("blocked_urls")?,
            });
        }
        Ok(listing_sources)
    }

    pub async fn get_review(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<ReviewDetail, ReviewRepositoryError> {
        let row = sqlx::query(
            "SELECT r.review_id, r.listing_source_id, s.listing_source_name, r.domain_id, r.artifact_type, r.status, r.reason,
                    r.candidate_payload, r.validation_summary, r.reviewer_notes, r.created, r.updated, r.reviewed
             FROM crawler_reviews r
             LEFT JOIN listing_sources s ON s.listing_source_id = r.listing_source_id
             WHERE r.review_id = $1",
        )
            .bind(review_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(ReviewRepositoryError::NotFound(review_id))?;

        let mut review = row_to_review(row)?;
        self.backfill_legacy_schema_review_payload(&mut review)
            .await?;
        let pages = self.get_review_pages(review_id).await?;
        let urls = self.get_review_urls(review_id).await?;
        Ok(ReviewDetail {
            review,
            pages,
            urls,
        })
    }

    pub async fn get_review_pages(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<Vec<CrawlerReviewPage>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT review_page_id, review_id, url, role, html_hash, fetched
             FROM crawler_review_pages
             WHERE review_id = $1
             ORDER BY
               CASE role
                 WHEN 'PRIMARY' THEN 0
                  WHEN 'TRIGGERING_GENERATION_PAGE' THEN 0
                  WHEN 'TRIGGERING_REPAIR_PAGE' THEN 0
                 ELSE 1
               END,
               created",
        )
        .bind(review_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_page).collect()
    }

    async fn get_review_urls(
        &self,
        review_id: uuid::Uuid,
    ) -> Result<Vec<CrawlerReviewUrl>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT review_url_id, review_id, url, previous_class, current_pattern_match,
                    candidate_pattern_match, candidate_class
             FROM crawler_review_urls
             WHERE review_id = $1
             ORDER BY created",
        )
        .bind(review_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(row_to_review_url).collect()
    }

    pub async fn get_review_page(
        &self,
        review_page_id: uuid::Uuid,
    ) -> Result<Option<CrawlerReviewPage>, sqlx::Error> {
        sqlx::query(
            "SELECT review_page_id, review_id, url, role, html_hash, fetched
             FROM crawler_review_pages
             WHERE review_page_id = $1",
        )
        .bind(review_page_id)
        .fetch_optional(&self.pool)
        .await?
        .map(row_to_page)
        .transpose()
    }

    pub async fn create_url_pattern_review(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        reason: &str,
        candidate_pattern: Option<&Regex>,
        urls: &[String],
        current_pattern: Option<&Regex>,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        let candidate_payload = serde_json::to_value(UrlPatternReviewCandidate::pattern(
            candidate_pattern,
            current_pattern,
        ))?;
        let validation_summary = json!({
            "sample_count": urls.len(),
            "candidate_product_count": candidate_pattern
                .map(|pattern| urls.iter().filter(|url| pattern.is_match(url)).count())
                .unwrap_or(0),
        });
        let mut transaction = self.pool.begin().await?;
        let domain_owned = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_source_domains \
             WHERE listing_source_id = $1 AND domain_id = $2 FOR KEY SHARE)",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(uuid::Uuid::from(*domain_id))
        .fetch_one(&mut *transaction)
        .await?;
        if !domain_owned {
            return Err(ReviewRepositoryError::Database(sqlx::Error::RowNotFound));
        }
        let inserted = sqlx::query_scalar::<_, uuid::Uuid>(
            "INSERT INTO crawler_reviews (listing_source_id, domain_id, artifact_type, status, reason, candidate_payload, validation_summary, reviewer_notes, reviewed) \
             VALUES ($1, $2, 'URL_PATTERN', 'PENDING_REVIEW', $3, $4, $5, NULL, NULL) \
             ON CONFLICT (domain_id) WHERE status = 'PENDING_REVIEW' AND artifact_type = 'URL_PATTERN' \
             DO NOTHING RETURNING review_id",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(uuid::Uuid::from(*domain_id))
        .bind(reason)
        .bind(candidate_payload)
        .bind(validation_summary)
        .fetch_optional(&mut *transaction)
        .await?;
        let (review_id, is_new) = match inserted {
            Some(review_id) => (review_id, true),
            None => (
                sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT review_id FROM crawler_reviews \
                     WHERE listing_source_id = $1 AND domain_id = $2 \
                       AND artifact_type = 'URL_PATTERN' AND status = 'PENDING_REVIEW' \
                     ORDER BY created DESC LIMIT 1",
                )
                .bind(uuid::Uuid::from(*listing_source_id))
                .bind(uuid::Uuid::from(*domain_id))
                .fetch_one(&mut *transaction)
                .await?,
                false,
            ),
        };
        if is_new {
            for raw_url in urls {
                let current_match = current_pattern.map(|pattern| pattern.is_match(raw_url));
                let candidate_match = candidate_pattern.map(|pattern| pattern.is_match(raw_url));
                let candidate_class = classify_with_pattern(raw_url, candidate_pattern);
                sqlx::query(
                    "INSERT INTO crawler_review_urls (
                        review_id, url, current_pattern_match, candidate_pattern_match, candidate_class
                     ) VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(review_id)
                .bind(raw_url)
                .bind(current_match)
                .bind(candidate_match)
                .bind(candidate_class)
                .execute(&mut *transaction)
                .await?;
            }
        }
        transaction.commit().await?;
        Ok(review_id)
    }

    pub async fn create_schema_review(
        &self,
        listing_source_id: &ListingSourceId,
        reason: &str,
        schemas: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
        validation_summary: serde_json::Value,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        self.insert_schema_review(SchemaReviewWithStatusInput {
            listing_source_id,
            reason,
            schemas,
            pages,
            validation_summary,
            status: STATUS_PENDING_REVIEW,
            notes: None,
        })
        .await
    }

    pub async fn create_schema_review_with_status(
        &self,
        input: SchemaReviewWithStatusInput<'_>,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        self.insert_schema_review(input).await
    }

    async fn insert_schema_review(
        &self,
        input: SchemaReviewWithStatusInput<'_>,
    ) -> Result<uuid::Uuid, ReviewRepositoryError> {
        let SchemaReviewWithStatusInput {
            listing_source_id,
            reason,
            schemas,
            pages,
            validation_summary,
            status,
            notes,
        } = input;
        validate_product_schemas(schemas)?;
        let candidate_payload = json!({ "schemas": schemas });
        let mut transaction = self.pool.begin().await?;
        let inserted = if status == STATUS_PENDING_REVIEW {
            sqlx::query_scalar::<_, uuid::Uuid>(
                "INSERT INTO crawler_reviews (listing_source_id, domain_id, artifact_type, status, reason, candidate_payload, validation_summary, reviewer_notes, reviewed) \
                 VALUES ($1, NULL, 'PRODUCT_SCHEMA', 'PENDING_REVIEW', $2, $3, $4, NULL, NULL) \
                 ON CONFLICT (listing_source_id) WHERE status = 'PENDING_REVIEW' AND artifact_type = 'PRODUCT_SCHEMA' \
                 DO NOTHING RETURNING review_id",
            )
            .bind(uuid::Uuid::from(*listing_source_id))
            .bind(reason)
            .bind(candidate_payload)
            .bind(validation_summary)
            .fetch_optional(&mut *transaction)
            .await?
        } else {
            Some(
                sqlx::query_scalar::<_, uuid::Uuid>(
                    "INSERT INTO crawler_reviews (listing_source_id, domain_id, artifact_type, status, reason, candidate_payload, validation_summary, reviewer_notes, reviewed) \
                     VALUES ($1, NULL, 'PRODUCT_SCHEMA', $2, $3, $4, $5, $6, CASE WHEN $2 = 'PENDING_REVIEW' THEN NULL ELSE NOW() END) \
                     RETURNING review_id",
                )
                .bind(uuid::Uuid::from(*listing_source_id))
                .bind(status)
                .bind(reason)
                .bind(candidate_payload)
                .bind(validation_summary)
                .bind(notes)
                .fetch_one(&mut *transaction)
                .await?,
            )
        };
        let (review_id, is_new) = match inserted {
            Some(review_id) => (review_id, true),
            None => (
                sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT review_id FROM crawler_reviews \
                     WHERE listing_source_id = $1 AND artifact_type = 'PRODUCT_SCHEMA' AND status = 'PENDING_REVIEW' \
                     ORDER BY created DESC LIMIT 1",
                )
                .bind(uuid::Uuid::from(*listing_source_id))
                .fetch_one(&mut *transaction)
                .await?,
                false,
            ),
        };
        if is_new {
            for page in pages {
                let html_hash = sha256_hex(page.raw_html.as_bytes());
                sqlx::query(
                    "INSERT INTO crawler_review_pages (review_id, url, role, html_hash) \
                     VALUES ($1, $2, $3, $4)",
                )
                .bind(review_id)
                .bind(page.url)
                .bind(page.role)
                .bind(html_hash)
                .execute(&mut *transaction)
                .await?;
            }
        }
        transaction.commit().await?;
        Ok(review_id)
    }

    pub async fn update_candidate_payload(
        &self,
        review_id: uuid::Uuid,
        candidate_payload: serde_json::Value,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let review = sqlx::query(
            "SELECT listing_source_id, artifact_type, status, validation_summary \
             FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
        )
        .bind(review_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let listing_source_id =
            ListingSourceId::from(review.try_get::<uuid::Uuid, _>("listing_source_id")?);
        let artifact_type: String = review.try_get("artifact_type")?;
        let status: String = review.try_get("status")?;
        let validation_summary: serde_json::Value = review.try_get("validation_summary")?;

        if status == STATUS_PENDING_REVIEW {
            if artifact_type == ARTIFACT_URL_PATTERN {
                let candidate =
                    serde_json::from_value::<UrlPatternReviewCandidate>(candidate_payload.clone())
                        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
                candidate
                    .validated_pattern()
                    .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
            }
            if artifact_type == ARTIFACT_PRODUCT_SCHEMA {
                parse_schemas_payload(&candidate_payload)?;
            }
            sqlx::query(
                "UPDATE crawler_reviews SET candidate_payload = $2, updated = NOW() \
                 WHERE review_id = $1",
            )
            .bind(review_id)
            .bind(candidate_payload)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
            return Ok(());
        }

        if status != STATUS_APPROVED || artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }

        let schemas = parse_schemas_payload(&candidate_payload)?;
        persist_product_schemas(&mut transaction, &listing_source_id, &schemas).await?;
        let validation_summary = append_manual_schema_edit(validation_summary, schemas.len());
        sqlx::query(
            "UPDATE crawler_reviews \
             SET candidate_payload = $2, validation_summary = $3, updated = NOW() \
             WHERE review_id = $1",
        )
        .bind(review_id)
        .bind(candidate_payload)
        .bind(validation_summary)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        info!(
            review_id = %review_id,
            listing_source_id = %listing_source_id,
            schema_count = schemas.len(),
            "Approved product schema edited from review console"
        );
        Ok(())
    }

    pub async fn update_schema_field(
        &self,
        review_id: uuid::Uuid,
        schema_index: usize,
        field: &str,
        rule: Option<ExtractionRule>,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let review = sqlx::query(
            "SELECT listing_source_id, artifact_type, status, reason, candidate_payload, validation_summary \
             FROM crawler_reviews WHERE review_id = $1 FOR UPDATE",
        )
        .bind(review_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let listing_source_id =
            ListingSourceId::from(review.try_get::<uuid::Uuid, _>("listing_source_id")?);
        let artifact_type: String = review.try_get("artifact_type")?;
        let status: String = review.try_get("status")?;
        let reason: String = review.try_get("reason")?;
        let candidate_payload: serde_json::Value = review.try_get("candidate_payload")?;
        let validation_summary: serde_json::Value = review.try_get("validation_summary")?;

        if artifact_type != ARTIFACT_PRODUCT_SCHEMA {
            return Err(ReviewRepositoryError::UnsupportedArtifact(
                review_id,
                artifact_type,
            ));
        }
        if !matches!(status.as_str(), STATUS_PENDING_REVIEW | STATUS_APPROVED) {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }

        let candidate_payload = if status == STATUS_PENDING_REVIEW
            && matches!(
                reason.as_str(),
                "append_schema_generation" | "normalization_schema_repair"
            ) {
            let reviewed_schemas = parse_schemas_payload(&candidate_payload)?;
            if reviewed_schemas.len() == 1 {
                let existing_payload = sqlx::query_scalar::<_, serde_json::Value>(
                    "SELECT product_schema FROM listing_source_product_schemas \
                     WHERE listing_source_id = $1 FOR UPDATE",
                )
                .bind(uuid::Uuid::from(listing_source_id))
                .fetch_optional(&mut *transaction)
                .await?;
                let existing = existing_payload
                    .map(|payload| {
                        parse_product_schema_payload(payload).map(|product_schemas| {
                            ListingSourceProductSchema {
                                listing_source_id,
                                product_schemas,
                                created: OffsetDateTime::UNIX_EPOCH,
                                updated: OffsetDateTime::UNIX_EPOCH,
                            }
                        })
                    })
                    .transpose()?;
                json!({
                    "schemas": approval_product_schemas(
                        &reason,
                        existing.as_ref(),
                        reviewed_schemas,
                    )?
                })
            } else {
                candidate_payload
            }
        } else {
            candidate_payload
        };
        let schemas = update_schema_field_payload(&candidate_payload, schema_index, field, rule)?;
        let candidate_payload = json!({ "schemas": schemas });

        if status == STATUS_APPROVED {
            persist_product_schemas(&mut transaction, &listing_source_id, &schemas).await?;
            let validation_summary = append_manual_schema_edit(validation_summary, schemas.len());
            sqlx::query(
                "UPDATE crawler_reviews \
                 SET candidate_payload = $2, validation_summary = $3, updated = NOW() \
                 WHERE review_id = $1",
            )
            .bind(review_id)
            .bind(candidate_payload)
            .bind(validation_summary)
            .execute(&mut *transaction)
            .await?;
        } else {
            sqlx::query(
                "UPDATE crawler_reviews SET candidate_payload = $2, updated = NOW() \
                 WHERE review_id = $1",
            )
            .bind(review_id)
            .bind(candidate_payload)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;

        if status == STATUS_APPROVED {
            info!(
                review_id = %review_id,
                listing_source_id = %listing_source_id,
                schema_count = schemas.len(),
                "Approved product schema edited from review console"
            );
        }
        Ok(())
    }

    pub async fn approve_review(
        &self,
        review_id: uuid::Uuid,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let review = sqlx::query(
            "SELECT listing_source_id, domain_id, artifact_type, reason, candidate_payload \
             FROM crawler_reviews \
             WHERE review_id = $1 AND status = 'PENDING_REVIEW' \
             FOR UPDATE",
        )
        .bind(review_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ReviewRepositoryError::NotPending(review_id))?;
        let listing_source_id =
            ListingSourceId::from(review.try_get::<uuid::Uuid, _>("listing_source_id")?);
        let domain_id: Option<CrawlerDomainId> = review
            .try_get::<Option<uuid::Uuid>, _>("domain_id")?
            .map(Into::into);
        let artifact_type: String = review.try_get("artifact_type")?;
        let reason: String = review.try_get("reason")?;
        let candidate_payload: serde_json::Value = review.try_get("candidate_payload")?;

        match artifact_type.as_str() {
            ARTIFACT_URL_PATTERN => {
                let candidate =
                    serde_json::from_value::<UrlPatternReviewCandidate>(candidate_payload)
                        .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
                let pattern = candidate
                    .validated_pattern()
                    .map_err(|_| ReviewRepositoryError::InvalidUrlPatternCandidate)?;
                let domain_id = domain_id.ok_or(sqlx::Error::RowNotFound)?;
                let result = sqlx::query(
                    "UPDATE listing_source_domains \
                     SET url_pattern = $3, \
                         url_pattern_state = CASE WHEN $3 IS NULL THEN 'NO_PATTERN' ELSE 'MATCHED' END \
                     WHERE listing_source_id = $1 AND domain_id = $2",
                )
                .bind(uuid::Uuid::from(listing_source_id))
                .bind(uuid::Uuid::from(domain_id))
                .bind(pattern)
                .execute(&mut *transaction)
                .await?;
                if result.rows_affected() == 0 {
                    return Err(ReviewRepositoryError::NotFound(review_id));
                }
            }
            ARTIFACT_PRODUCT_SCHEMA => {
                let reviewed_schemas = parse_schemas_payload(&candidate_payload)?;
                let existing_payload = sqlx::query_scalar::<_, serde_json::Value>(
                    "SELECT product_schema FROM listing_source_product_schemas \
                     WHERE listing_source_id = $1 FOR UPDATE",
                )
                .bind(uuid::Uuid::from(listing_source_id))
                .fetch_optional(&mut *transaction)
                .await?;
                let existing = existing_payload
                    .map(|payload| {
                        parse_product_schema_payload(payload).map(|product_schemas| {
                            ListingSourceProductSchema {
                                listing_source_id,
                                product_schemas,
                                created: OffsetDateTime::UNIX_EPOCH,
                                updated: OffsetDateTime::UNIX_EPOCH,
                            }
                        })
                    })
                    .transpose()?;
                let schemas =
                    approval_product_schemas(&reason, existing.as_ref(), reviewed_schemas)?;
                persist_product_schemas(&mut transaction, &listing_source_id, &schemas).await?;
            }
            other => {
                return Err(ReviewRepositoryError::UnsupportedArtifact(
                    review_id,
                    other.into(),
                ));
            }
        }

        sqlx::query(
            "UPDATE crawler_reviews \
             SET status = 'APPROVED', reviewer_notes = COALESCE($2, reviewer_notes), \
                 reviewed = NOW(), updated = NOW() \
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id)
        .bind(notes)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "UPDATE crawler_reviews \
             SET status = 'SUPERSEDED', reviewed = NOW(), updated = NOW() \
             WHERE listing_source_id = $1 AND artifact_type = $2 \
               AND status = 'PENDING_REVIEW' AND review_id <> $3 \
               AND ($4::uuid IS NULL OR domain_id = $4)",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(&artifact_type)
        .bind(review_id)
        .bind(domain_id.map(uuid::Uuid::from))
        .execute(&mut *transaction)
        .await?;
        match artifact_type.as_str() {
            ARTIFACT_PRODUCT_SCHEMA => {
                sqlx::query(
                    "UPDATE listing_source_urls \
                     SET next_retry_at = NULL, last_error_kind = NULL, \
                         last_error_message = NULL, updated = NOW() \
                     WHERE listing_source_id = $1 \
                       AND last_error_kind = 'PendingSchemaReview' \
                       AND crawler_disposition = 'ACTIVE'",
                )
                .bind(uuid::Uuid::from(listing_source_id))
                .execute(&mut *transaction)
                .await?;
            }
            ARTIFACT_URL_PATTERN => {
                sqlx::query(
                    "UPDATE listing_source_domains \
                     SET next_crawl_at = NULL, last_crawl_error_kind = NULL \
                     WHERE listing_source_id = $1 AND domain_id = $2 \
                       AND last_crawl_error_kind = 'PendingUrlPatternReview'",
                )
                .bind(uuid::Uuid::from(listing_source_id))
                .bind(domain_id.map(uuid::Uuid::from))
                .execute(&mut *transaction)
                .await?;
            }
            _ => {}
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn reject_review(
        &self,
        review_id: uuid::Uuid,
        notes: Option<&str>,
        needs_repair: bool,
    ) -> Result<(), ReviewRepositoryError> {
        let status = if needs_repair {
            STATUS_NEEDS_REPAIR
        } else {
            STATUS_REJECTED
        };
        self.set_review_status(review_id, status, notes).await
    }

    pub async fn evaluate_schema_matrix_for_live_pages(
        &self,
        review_id: uuid::Uuid,
        pages: Vec<(CrawlerReviewPage, String)>,
    ) -> Result<EvaluatedSchemaMatrix, ReviewRepositoryError> {
        let review = sqlx::query(
            "SELECT candidate_payload, candidate_version,
                    encode(digest(candidate_payload::text, 'sha256'), 'hex') AS candidate_hash
             FROM crawler_reviews
             WHERE review_id = $1",
        )
        .bind(review_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(ReviewRepositoryError::NotFound(review_id))?;
        let candidate_payload: serde_json::Value = review.try_get("candidate_payload")?;
        let schemas = parse_schemas_payload(&candidate_payload)?;
        Ok(EvaluatedSchemaMatrix {
            matrix: evaluate_schema_matrix_for_live_review_pages(review_id, &schemas, &pages),
            candidate_version: review.try_get("candidate_version")?,
            candidate_hash: review.try_get("candidate_hash")?,
        })
    }

    pub async fn store_schema_matrix_if_current(
        &self,
        review_id: uuid::Uuid,
        evaluated: &EvaluatedSchemaMatrix,
    ) -> Result<(), ReviewRepositoryError> {
        let matrix = serde_json::to_value(&evaluated.matrix)?;
        let result = sqlx::query(
            "UPDATE crawler_reviews
             SET validation_summary = jsonb_set(
                     CASE jsonb_typeof(validation_summary)
                         WHEN 'object' THEN validation_summary
                         ELSE jsonb_build_object('summary', validation_summary)
                     END,
                     '{schema_matrix}',
                     $2::jsonb,
                     true
                 ),
                 updated = NOW()
             WHERE review_id = $1
               AND candidate_version = $3
               AND encode(digest(candidate_payload::text, 'sha256'), 'hex') = $4",
        )
        .bind(review_id)
        .bind(matrix)
        .bind(evaluated.candidate_version)
        .bind(&evaluated.candidate_hash)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(ReviewRepositoryError::CandidateChangedDuringEvaluation);
        }
        Ok(())
    }

    async fn backfill_legacy_schema_review_payload(
        &self,
        review: &mut CrawlerReview,
    ) -> Result<(), ReviewRepositoryError> {
        if review.artifact_type != ARTIFACT_PRODUCT_SCHEMA
            || review.status != STATUS_PENDING_REVIEW
            || !matches!(
                review.reason.as_str(),
                "append_schema_generation" | "normalization_schema_repair"
            )
        {
            return Ok(());
        }

        let reviewed_schemas = parse_schemas_payload(&review.candidate_payload)?;
        if reviewed_schemas.len() != 1 {
            return Ok(());
        }

        let repo = ListingSourceProductSchemaRepositoryImpl::new(&self.pool);
        let Some(existing) = repo.find_product_schema(&review.listing_source_id).await? else {
            return Ok(());
        };
        let merged = approval_product_schemas(&review.reason, Some(&existing), reviewed_schemas)?;
        review.candidate_payload = json!({ "schemas": merged });
        Ok(())
    }

    async fn set_review_status(
        &self,
        review_id: uuid::Uuid,
        status: &str,
        notes: Option<&str>,
    ) -> Result<(), ReviewRepositoryError> {
        let result = sqlx::query(
            "UPDATE crawler_reviews
             SET status = $2, reviewer_notes = COALESCE($3, reviewer_notes),
                 reviewed = NOW(), updated = NOW()
             WHERE review_id = $1 AND status = 'PENDING_REVIEW'",
        )
        .bind(review_id)
        .bind(status)
        .bind(notes)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(ReviewRepositoryError::NotPending(review_id));
        }
        Ok(())
    }
}

async fn persist_product_schemas(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    listing_source_id: &ListingSourceId,
    schemas: &[ProductCssSelectorSchema],
) -> Result<(), ReviewRepositoryError> {
    let product_schema = serde_json::to_value(schemas)?;
    sqlx::query(
        "INSERT INTO listing_source_product_schemas (listing_source_id, product_schema, created, updated) \
         VALUES ($1, $2, NOW(), NOW()) \
         ON CONFLICT (listing_source_id) DO UPDATE \
         SET product_schema = EXCLUDED.product_schema, updated = NOW()",
    )
    .bind(uuid::Uuid::from(*listing_source_id))
    .bind(product_schema)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn parse_product_schema_payload(
    payload: serde_json::Value,
) -> Result<Vec<ProductCssSelectorSchema>, ReviewRepositoryError> {
    match serde_json::from_value::<Vec<ProductCssSelectorSchema>>(payload.clone()) {
        Ok(schemas) => Ok(schemas),
        Err(_) => Ok(vec![serde_json::from_value(payload)?]),
    }
}

fn row_to_review(row: sqlx::postgres::PgRow) -> Result<CrawlerReview, sqlx::Error> {
    Ok(CrawlerReview {
        review_id: row.try_get("review_id")?,
        listing_source_id: ListingSourceId::from(
            row.try_get::<uuid::Uuid, _>("listing_source_id")?,
        ),
        listing_source_name: row.try_get("listing_source_name")?,
        domain_id: row
            .try_get::<Option<uuid::Uuid>, _>("domain_id")?
            .map(Into::into),
        artifact_type: row.try_get("artifact_type")?,
        status: row.try_get("status")?,
        reason: row.try_get("reason")?,
        candidate_payload: row.try_get("candidate_payload")?,
        validation_summary: row.try_get("validation_summary")?,
        reviewer_notes: row.try_get("reviewer_notes")?,
        created: row.try_get("created")?,
        updated: row.try_get("updated")?,
        reviewed: row.try_get("reviewed")?,
    })
}

fn row_to_page(row: sqlx::postgres::PgRow) -> Result<CrawlerReviewPage, sqlx::Error> {
    Ok(CrawlerReviewPage {
        review_page_id: row.try_get("review_page_id")?,
        review_id: row.try_get("review_id")?,
        url: row.try_get("url")?,
        role: row.try_get("role")?,
        html_hash: row.try_get("html_hash")?,
        fetched: row.try_get("fetched")?,
    })
}

fn row_to_review_url(row: sqlx::postgres::PgRow) -> Result<CrawlerReviewUrl, sqlx::Error> {
    Ok(CrawlerReviewUrl {
        review_url_id: row.try_get("review_url_id")?,
        review_id: row.try_get("review_id")?,
        url: row.try_get("url")?,
        previous_class: row.try_get("previous_class")?,
        current_pattern_match: row.try_get("current_pattern_match")?,
        candidate_pattern_match: row.try_get("candidate_pattern_match")?,
        candidate_class: row.try_get("candidate_class")?,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn append_manual_schema_edit(
    mut validation_summary: serde_json::Value,
    schema_count: usize,
) -> serde_json::Value {
    let edit = json!({
        "at": OffsetDateTime::now_utc().to_string(),
        "source": "review_console",
        "operation": "approved_schema_live_update",
        "schema_count": schema_count,
    });

    if !validation_summary.is_object() {
        validation_summary = json!({ "summary": validation_summary });
    }

    if let Some(object) = validation_summary.as_object_mut() {
        let edits = object
            .entry("manual_schema_edits")
            .or_insert_with(|| json!([]));
        if let Some(edits) = edits.as_array_mut() {
            edits.push(edit);
        } else {
            *edits = json!([edit]);
        }
    }
    validation_summary
}

fn classify_with_pattern(raw_url: &str, pattern: Option<&Regex>) -> String {
    let Ok(url) = Url::parse(raw_url) else {
        return "other".to_string();
    };
    CrawledUrl::new(url).classify(pattern).to_string()
}

#[cfg(test)]
mod tests {
    use super::append_manual_schema_edit;
    use serde_json::json;

    #[test]
    fn appends_manual_schema_edit_audit_entry() {
        let summary = json!({ "auto_schema_evaluation": { "confidence": "HIGH" } });

        let updated = append_manual_schema_edit(summary, 2);

        let edits = updated
            .get("manual_schema_edits")
            .and_then(serde_json::Value::as_array)
            .expect("manual edits should be an array");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["source"], "review_console");
        assert_eq!(edits[0]["operation"], "approved_schema_live_update");
        assert_eq!(edits[0]["schema_count"], 2);
        assert_eq!(
            updated["auto_schema_evaluation"]["confidence"],
            json!("HIGH")
        );
    }
}

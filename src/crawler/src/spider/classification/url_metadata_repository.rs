use crate::CrawlerDomainId;
use crate::network::policy::{
    PublicTargetError, url_matches_configured_domain, validate_public_http_url,
};
use crate::spider::classification::url_metadata::{
    CrawlerDisposition, CrawlerUrlWriteOutcome, UrlClass,
};
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct SpiderUrlRecord {
    pub listing_source_id: ListingSourceId,
    pub domain_id: CrawlerDomainId,
    pub url: url::Url,
    pub url_class: UrlClass,
    pub disposition: CrawlerDisposition,
    pub last_scraped_hash: Option<String>,
    pub last_scraped: Option<OffsetDateTime>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

impl FromRow<'_, sqlx::postgres::PgRow> for SpiderUrlRecord {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let listing_source_id: uuid::Uuid = row.try_get("listing_source_id")?;
        let domain_id: uuid::Uuid = row.try_get("domain_id")?;
        let url = url::Url::parse(&row.try_get::<String, _>("url")?)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?;
        let url_class = row
            .try_get::<String, _>("url_class")?
            .parse()
            .map_err(|error: String| sqlx::Error::Decode(error.into()))?;
        let disposition = row
            .try_get::<String, _>("crawler_disposition")?
            .parse()
            .map_err(|error: String| sqlx::Error::Decode(error.into()))?;
        Ok(Self {
            listing_source_id: listing_source_id.into(),
            domain_id: domain_id.into(),
            url,
            url_class,
            disposition,
            last_scraped_hash: row.try_get("last_scraped_hash")?,
            last_scraped: row.try_get("last_scraped")?,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UrlMetadataRepositoryError {
    #[error("crawler domain does not belong to ListingSource")]
    DomainNotOwnedByListingSource {
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    },
    #[error("URL is not a supported crawler HTTP URL")]
    InvalidCrawlerUrl {
        url: url::Url,
        reason: PublicTargetError,
    },
    #[error("URL host does not match the configured crawler domain")]
    UrlHostDoesNotMatchDomain { url: url::Url, domain: String },
    #[error("URL is already owned by another ListingSource")]
    UrlOwnedByAnotherListingSource {
        url: url::Url,
        requested_listing_source_id: ListingSourceId,
        current_listing_source_id: ListingSourceId,
    },
    #[error("URL is already owned by another crawler domain")]
    UrlOwnedByAnotherDomain {
        url: url::Url,
        current_domain_id: CrawlerDomainId,
        requested_domain_id: CrawlerDomainId,
    },
    #[error("crawler URL persistence failed")]
    Database {
        #[source]
        source: sqlx::Error,
    },
}

#[async_trait]
#[mockall::automock]
pub trait UrlMetadataRepository: Send + Sync {
    async fn upsert_link(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, UrlMetadataRepositoryError>;

    async fn upsert_links_batch(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        urls: &[url::Url],
        url_classes: &[UrlClass],
    ) -> Result<Vec<SpiderUrlRecord>, UrlMetadataRepositoryError>;

    async fn mark_as_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        hash: &str,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;

    async fn set_disposition(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        disposition: CrawlerDisposition,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error>;
}

pub struct UrlMetadataRepositoryImpl {
    pool: PgPool,
}

impl UrlMetadataRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn verify_domain_owner(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    ) -> Result<String, UrlMetadataRepositoryError> {
        let domain = sqlx::query_scalar::<_, String>(
            "SELECT listing_source_domain FROM listing_source_domains \
             WHERE listing_source_id = $1 AND domain_id = $2 \
             FOR KEY SHARE",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(uuid::Uuid::from(domain_id))
        .fetch_optional(&mut **transaction)
        .await
        .map_err(database_error)?;
        domain.ok_or(UrlMetadataRepositoryError::DomainNotOwnedByListingSource {
            listing_source_id,
            domain_id,
        })
    }

    async fn validate_existing_url_ownership(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
        urls: &[String],
    ) -> Result<(), UrlMetadataRepositoryError> {
        let rows = sqlx::query(
            "SELECT url, listing_source_id, domain_id FROM listing_source_urls \
             WHERE url = ANY($1::text[]) ORDER BY url FOR UPDATE",
        )
        .bind(urls)
        .fetch_all(&mut **transaction)
        .await
        .map_err(database_error)?;

        for row in rows {
            let raw_url = row.try_get::<String, _>("url").map_err(database_error)?;
            let url = url::Url::parse(&raw_url).map_err(|error| {
                UrlMetadataRepositoryError::Database {
                    source: sqlx::Error::Decode(Box::new(error)),
                }
            })?;
            let current_listing_source_id: ListingSourceId = row
                .try_get::<uuid::Uuid, _>("listing_source_id")
                .map_err(database_error)?
                .into();
            if current_listing_source_id != listing_source_id {
                return Err(UrlMetadataRepositoryError::UrlOwnedByAnotherListingSource {
                    url,
                    requested_listing_source_id: listing_source_id,
                    current_listing_source_id,
                });
            }
            let current_domain_id = row
                .try_get::<uuid::Uuid, _>("domain_id")
                .map_err(database_error)?
                .into();
            if current_domain_id != domain_id {
                return Err(UrlMetadataRepositoryError::UrlOwnedByAnotherDomain {
                    url,
                    current_domain_id,
                    requested_domain_id: domain_id,
                });
            }
        }
        Ok(())
    }

    fn verify_url_hosts(urls: &[url::Url], domain: &str) -> Result<(), UrlMetadataRepositoryError> {
        for url in urls {
            if url.fragment().is_some() {
                return Err(UrlMetadataRepositoryError::InvalidCrawlerUrl {
                    url: url.clone(),
                    reason: PublicTargetError::InvalidUrl,
                });
            }
            if let Err(reason) = validate_public_http_url(url) {
                return Err(UrlMetadataRepositoryError::InvalidCrawlerUrl {
                    url: url.clone(),
                    reason,
                });
            }
        }
        urls.iter()
            .find(|url| !url_matches_configured_domain(url, domain))
            .map_or(Ok(()), |url| {
                Err(UrlMetadataRepositoryError::UrlHostDoesNotMatchDomain {
                    url: url.clone(),
                    domain: domain.to_string(),
                })
            })
    }
}

#[async_trait]
impl UrlMetadataRepository for UrlMetadataRepositoryImpl {
    async fn upsert_link(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        url: &url::Url,
        url_class: &UrlClass,
    ) -> Result<SpiderUrlRecord, UrlMetadataRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let domain =
            Self::verify_domain_owner(&mut transaction, *listing_source_id, *domain_id).await?;
        Self::verify_url_hosts(std::slice::from_ref(url), &domain)?;
        let urls = vec![url.to_string()];
        Self::validate_existing_url_ownership(
            &mut transaction,
            *listing_source_id,
            *domain_id,
            &urls,
        )
        .await?;

        let record = sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO listing_source_urls (listing_source_id, domain_id, url, url_class, created, updated) \
             VALUES ($1, $2, $3, $4, NOW(), NOW()) \
             ON CONFLICT (url) DO UPDATE SET \
                 url_class = CASE \
                     WHEN listing_source_urls.crawler_disposition = 'ACTIVE' THEN EXCLUDED.url_class \
                     ELSE listing_source_urls.url_class \
                 END, \
                 updated = CASE \
                     WHEN listing_source_urls.crawler_disposition = 'ACTIVE' THEN NOW() \
                     ELSE listing_source_urls.updated \
                 END \
             WHERE listing_source_urls.listing_source_id = EXCLUDED.listing_source_id \
               AND listing_source_urls.domain_id = EXCLUDED.domain_id \
             RETURNING listing_source_id, domain_id, url, url_class, crawler_disposition, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(uuid::Uuid::from(*domain_id))
        .bind(url.as_str())
        .bind(url_class.to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;

        let Some(record) = record else {
            Self::validate_existing_url_ownership(
                &mut transaction,
                *listing_source_id,
                *domain_id,
                &urls,
            )
            .await?;
            return Err(UrlMetadataRepositoryError::Database {
                source: sqlx::Error::RowNotFound,
            });
        };
        transaction.commit().await.map_err(database_error)?;
        Ok(record)
    }

    async fn upsert_links_batch(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: &CrawlerDomainId,
        urls: &[url::Url],
        url_classes: &[UrlClass],
    ) -> Result<Vec<SpiderUrlRecord>, UrlMetadataRepositoryError> {
        if urls.is_empty() {
            return Ok(Vec::new());
        }
        if urls.len() != url_classes.len() {
            return Err(UrlMetadataRepositoryError::Database {
                source: sqlx::Error::Protocol("URL and URL-class batch lengths differ".to_owned()),
            });
        }

        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let domain =
            Self::verify_domain_owner(&mut transaction, *listing_source_id, *domain_id).await?;
        Self::verify_url_hosts(urls, &domain)?;
        let url_strings = urls.iter().map(ToString::to_string).collect::<Vec<_>>();
        Self::validate_existing_url_ownership(
            &mut transaction,
            *listing_source_id,
            *domain_id,
            &url_strings,
        )
        .await?;

        let records = sqlx::query_as::<_, SpiderUrlRecord>(
            "INSERT INTO listing_source_urls (listing_source_id, domain_id, url, url_class, created, updated) \
             SELECT $1, $2, input.url, input.url_class, NOW(), NOW() \
             FROM UNNEST($3::text[], $4::text[]) AS input(url, url_class) \
             ON CONFLICT (url) DO UPDATE SET \
                 url_class = CASE \
                     WHEN listing_source_urls.crawler_disposition = 'ACTIVE' THEN EXCLUDED.url_class \
                     ELSE listing_source_urls.url_class \
                 END, \
                 updated = CASE \
                     WHEN listing_source_urls.crawler_disposition = 'ACTIVE' THEN NOW() \
                     ELSE listing_source_urls.updated \
                 END \
             WHERE listing_source_urls.listing_source_id = EXCLUDED.listing_source_id \
               AND listing_source_urls.domain_id = EXCLUDED.domain_id \
             RETURNING listing_source_id, domain_id, url, url_class, crawler_disposition, last_scraped_hash, last_scraped, created, updated",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(uuid::Uuid::from(*domain_id))
        .bind(&url_strings)
        .bind(url_classes.iter().map(ToString::to_string).collect::<Vec<_>>())
        .fetch_all(&mut *transaction)
        .await
        .map_err(database_error)?;

        if records.len() != urls.len() {
            Self::validate_existing_url_ownership(
                &mut transaction,
                *listing_source_id,
                *domain_id,
                &url_strings,
            )
            .await?;
            return Err(UrlMetadataRepositoryError::Database {
                source: sqlx::Error::Protocol(
                    "crawler URL batch returned fewer rows than inputs".to_owned(),
                ),
            });
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(records)
    }

    async fn mark_as_scraped(
        &self,
        listing_source_id: &ListingSourceId,
        url: &url::Url,
        hash: &str,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_urls \
             SET last_scraped = NOW(), last_scraped_hash = $3, updated = NOW() \
             WHERE listing_source_id = $1 \
               AND url = $2 \
               AND crawler_disposition = 'ACTIVE'",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(url.as_str())
        .bind(hash)
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
        url: &url::Url,
        disposition: CrawlerDisposition,
    ) -> Result<CrawlerUrlWriteOutcome, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE listing_source_urls \
             SET crawler_disposition = $3, updated = NOW() \
             WHERE listing_source_id = $1 \
               AND url = $2 \
               AND crawler_disposition = 'ACTIVE'",
        )
        .bind(uuid::Uuid::from(*listing_source_id))
        .bind(url.as_str())
        .bind(disposition.as_str())
        .execute(&self.pool)
        .await?;

        Ok(if result.rows_affected() == 1 {
            CrawlerUrlWriteOutcome::Applied
        } else {
            CrawlerUrlWriteOutcome::NoopStale
        })
    }
}

fn database_error(source: sqlx::Error) -> UrlMetadataRepositoryError {
    UrlMetadataRepositoryError::Database { source }
}

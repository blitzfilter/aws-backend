use crate::CrawlerDomainId;
use crate::network::policy::canonical_crawler_domain;
use crate::service::crawler_domain_configuration::{
    CrawlerDomainConfiguration, CrawlerDomainConfigurationError,
    CrawlerDomainConfigurationRepository, CrawlerDomainRemoval,
};
use application::error::box_error;
use async_trait::async_trait;
use listing_source_core::{Domain, ListingSourceId};
use sqlx::PgPool;
use std::net::IpAddr;

#[derive(Clone)]
pub struct CrawlerDomainConfigurationRepositoryImpl {
    pool: PgPool,
}

impl CrawlerDomainConfigurationRepositoryImpl {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CrawlerDomainConfigurationRepository for CrawlerDomainConfigurationRepositoryImpl {
    async fn list_for_source(
        &self,
        listing_source_id: ListingSourceId,
    ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError> {
        let source_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?;
        if !source_exists {
            return Err(CrawlerDomainConfigurationError::ListingSourceNotFound {
                listing_source_id,
            });
        }
        let rows = sqlx::query_as::<_, (uuid::Uuid, String)>(
            "SELECT domain_id, crawl_root_host \
             FROM listing_source_domains \
             WHERE listing_source_id = $1 \
             ORDER BY listing_source_domain",
        )
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;

        rows.into_iter()
            .map(|(domain_id, raw_domain)| {
                let domain_id = persisted_domain_id(domain_id)?;
                let domain = Domain::try_from(raw_domain).map_err(|source| {
                    CrawlerDomainConfigurationError::Database {
                        source: box_error(source),
                    }
                })?;
                Ok(CrawlerDomainConfiguration {
                    domain_id,
                    listing_source_id,
                    domain,
                    created: false,
                })
            })
            .collect()
    }

    async fn register(
        &self,
        listing_source_id: ListingSourceId,
        domain: Domain,
    ) -> Result<CrawlerDomainConfiguration, CrawlerDomainConfigurationError> {
        let normalized_domain = domain.as_str().trim_end_matches('.').to_ascii_lowercase();
        if normalized_domain.starts_with("www.www.") {
            return Err(CrawlerDomainConfigurationError::RepeatedWwwPrefix { domain });
        }
        let crawl_root_host = Domain::try_from(normalized_domain).map_err(|source| {
            CrawlerDomainConfigurationError::Database {
                source: box_error(source),
            }
        })?;
        let canonical_domain = Domain::try_from(canonical_crawler_domain(crawl_root_host.as_str()))
            .map_err(|source| CrawlerDomainConfigurationError::Database {
                source: box_error(source),
            })?;
        if crawl_root_host.as_str().parse::<IpAddr>().is_ok() {
            return Err(CrawlerDomainConfigurationError::UnsafeDomain { domain });
        }
        let listing_source_id_uuid = uuid::Uuid::from(listing_source_id);
        let mut transaction = self.pool.begin().await.map_err(database_error)?;

        // A row lock alone cannot protect a domain that is not inserted yet.
        // Serialize registration by canonical domain so concurrent same-source
        // registration remains idempotent and cross-source ownership is stable.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(canonical_domain.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(database_error)?;

        let source_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS ( \
                SELECT 1 FROM listing_sources \
                WHERE listing_source_id = $1 FOR KEY SHARE \
             )",
        )
        .bind(listing_source_id_uuid)
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        if !source_exists {
            return Err(CrawlerDomainConfigurationError::ListingSourceNotFound {
                listing_source_id,
            });
        }

        let existing_owner = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT listing_source_id FROM listing_source_domains \
             WHERE regexp_replace(lower(rtrim(listing_source_domain, '.')), '^www[.]', '') = $1 \
             FOR UPDATE",
        )
        .bind(canonical_domain.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?;
        let requested_domain_id = CrawlerDomainId::new();
        let (domain_id, registered_root_host, created) = match existing_owner {
            Some(owner) if owner == listing_source_id_uuid => {
                let (domain_id, root_host) = sqlx::query_as::<_, (uuid::Uuid, String)>(
                    "SELECT domain_id, crawl_root_host FROM listing_source_domains \
                     WHERE regexp_replace(lower(rtrim(listing_source_domain, '.')), '^www[.]', '') = $1",
                )
                .bind(canonical_domain.as_str())
                .fetch_one(&mut *transaction)
                .await
                .map_err(database_error)?;
                let root_host = Domain::try_from(root_host).map_err(|source| {
                    CrawlerDomainConfigurationError::Database {
                        source: box_error(source),
                    }
                })?;
                (persisted_domain_id(domain_id)?, root_host, false)
            }
            Some(owner) => {
                return Err(
                    CrawlerDomainConfigurationError::DomainOwnedByAnotherListingSource {
                        domain,
                        requested_listing_source_id: listing_source_id,
                        current_listing_source_id: persisted_listing_source_id(owner)?,
                    },
                );
            }
            None => {
                sqlx::query(
                    "INSERT INTO listing_source_domains \
                     (domain_id, listing_source_id, listing_source_domain, crawl_root_host) \
                     VALUES ($1, $2, $3, $4)",
                )
                .bind(requested_domain_id.as_uuid())
                .bind(listing_source_id_uuid)
                .bind(canonical_domain.as_str())
                .bind(crawl_root_host.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(database_error)?;
                (requested_domain_id, crawl_root_host, true)
            }
        };

        transaction.commit().await.map_err(database_error)?;
        Ok(CrawlerDomainConfiguration {
            domain_id,
            listing_source_id,
            domain: registered_root_host,
            created,
        })
    }

    async fn remove(
        &self,
        listing_source_id: ListingSourceId,
        domain_id: CrawlerDomainId,
    ) -> Result<CrawlerDomainRemoval, CrawlerDomainConfigurationError> {
        let mut transaction = self.pool.begin().await.map_err(database_error)?;
        let listing_source_id_uuid = uuid::Uuid::from(listing_source_id);
        let owned_domain = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT domain_id FROM listing_source_domains \
             WHERE listing_source_id = $1 AND domain_id = $2 \
             FOR UPDATE",
        )
        .bind(listing_source_id_uuid)
        .bind(domain_id.as_uuid())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(database_error)?
        .map(persisted_domain_id)
        .transpose()?;
        if owned_domain.is_none() {
            return Err(
                CrawlerDomainConfigurationError::DomainNotOwnedByListingSource {
                    listing_source_id,
                    domain_id,
                },
            );
        }

        let removed_url_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM listing_source_urls \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(listing_source_id_uuid)
        .bind(domain_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;
        let removed_url_pattern_review_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM crawler_reviews \
             WHERE listing_source_id = $1 AND domain_id = $2 \
               AND artifact_type = 'URL_PATTERN'",
        )
        .bind(listing_source_id_uuid)
        .bind(domain_id.as_uuid())
        .fetch_one(&mut *transaction)
        .await
        .map_err(database_error)?;

        sqlx::query(
            "DELETE FROM listing_source_domains \
             WHERE listing_source_id = $1 AND domain_id = $2",
        )
        .bind(listing_source_id_uuid)
        .bind(domain_id.as_uuid())
        .execute(&mut *transaction)
        .await
        .map_err(database_error)?;

        transaction.commit().await.map_err(database_error)?;
        Ok(CrawlerDomainRemoval {
            domain_id,
            removed_url_count,
            removed_url_pattern_review_count,
        })
    }
}

fn persisted_listing_source_id(
    value: uuid::Uuid,
) -> Result<ListingSourceId, CrawlerDomainConfigurationError> {
    ListingSourceId::try_from(value).map_err(|source| CrawlerDomainConfigurationError::Database {
        source: box_error(source),
    })
}

fn persisted_domain_id(
    value: uuid::Uuid,
) -> Result<CrawlerDomainId, CrawlerDomainConfigurationError> {
    CrawlerDomainId::try_from(value).map_err(|source| CrawlerDomainConfigurationError::Database {
        source: box_error(source),
    })
}

fn database_error(source: sqlx::Error) -> CrawlerDomainConfigurationError {
    CrawlerDomainConfigurationError::Database {
        source: box_error(source),
    }
}

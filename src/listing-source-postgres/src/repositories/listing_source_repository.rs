use application::{error::box_error, patch_field::PatchField};
use listing_source_core::*;
use listing_source_service::ports::*;

use sqlx::PgConnection;
use std::collections::HashSet;

use time::OffsetDateTime;
use url::Url;

pub(crate) struct SqlxListingSourceRepository<'a> {
    pub(crate) connection: &'a mut PgConnection,
}
#[derive(sqlx::FromRow)]
struct SourceRow {
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    name: String,
    operator_party_id: uuid::Uuid,
    url: Option<String>,
    image: Option<String>,
    referral_configuration: Option<serde_json::Value>,
    version: i64,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}
#[derive(sqlx::FromRow)]
struct MethodRow {
    ingestion_method: String,
}
#[async_trait::async_trait]
impl ListingSourceRepository for SqlxListingSourceRepository<'_> {
    async fn find_by_id(
        &mut self,
        id: ListingSourceId,
    ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
        let row=sqlx::query_as::<_,SourceRow>("SELECT listing_source_id, listing_source_slug_id, name, operator_party_id, url, image, referral_configuration, version, created, updated FROM listing_sources WHERE listing_source_id=$1").bind(uuid::Uuid::from(id)).fetch_optional(&mut *self.connection).await.map_err(db_read)?;
        match row {
            Some(row) => load(self.connection, row).await.map(Some),
            None => Ok(None),
        }
    }
    async fn find_by_slug(
        &mut self,
        slug: &ListingSourceSlugId,
    ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
        let row=sqlx::query_as::<_,SourceRow>("SELECT listing_source_id, listing_source_slug_id, name, operator_party_id, url, image, referral_configuration, version, created, updated FROM listing_sources WHERE listing_source_slug_id=$1").bind(slug.as_ref()).fetch_optional(&mut *self.connection).await.map_err(db_read)?;
        match row {
            Some(row) => load(self.connection, row).await.map(Some),
            None => Ok(None),
        }
    }
    async fn find_by_id_for_update(
        &mut self,
        id: ListingSourceId,
    ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
        let row = sqlx::query_as::<_, SourceRow>(
            "SELECT listing_source_id, listing_source_slug_id, name, operator_party_id, url, image, referral_configuration, version, created, updated FROM listing_sources WHERE listing_source_id=$1 FOR UPDATE",
        )
        .bind(uuid::Uuid::from(id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(db_read)?;
        match row {
            Some(row) => load(self.connection, row).await.map(Some),
            None => Ok(None),
        }
    }
    async fn find_deletion_blocker(
        &mut self,
        id: ListingSourceId,
    ) -> Result<Option<ListingSourceDeletionBlocker>, ListingSourceRepositoryError> {
        let blocker = sqlx::query_scalar::<_, Option<String>>(
            r#"
            SELECT CASE
                WHEN EXISTS (SELECT 1 FROM product_listings WHERE listing_source_id = $1)
                    THEN 'PRODUCT_LISTINGS'
                WHEN EXISTS (SELECT 1 FROM product_listing_raw_streams WHERE listing_source_id = $1)
                    THEN 'RAW_STREAMS'
                WHEN EXISTS (SELECT 1 FROM partnership_applications WHERE approved_listing_source_id = $1)
                    THEN 'APPROVED_APPLICATION'
                WHEN EXISTS (
                    SELECT 1 FROM partnership_applications
                    WHERE proposal->>'type' = 'EXISTING_LISTING_SOURCE'
                      AND proposal->>'listing_source_id' = $2
                ) THEN 'EXISTING_SOURCE_APPLICATION'
                ELSE NULL
            END
            "#,
        )
        .bind(uuid::Uuid::from(id))
        .bind(id.to_string())
        .fetch_one(&mut *self.connection)
        .await
        .map_err(db_read)?;
        match blocker.as_deref() {
            None => Ok(None),
            Some("PRODUCT_LISTINGS") => Ok(Some(ListingSourceDeletionBlocker::ProductListings)),
            Some("RAW_STREAMS") => Ok(Some(ListingSourceDeletionBlocker::RawStreams)),
            Some("APPROVED_APPLICATION") => Ok(Some(
                ListingSourceDeletionBlocker::ApprovedPartnershipApplication,
            )),
            Some("EXISTING_SOURCE_APPLICATION") => Ok(Some(
                ListingSourceDeletionBlocker::ExistingSourcePartnershipApplication,
            )),
            Some(_) => Err(ListingSourceRepositoryError::InvalidPersistedState {
                source: box_error(ListingIngestionConfigurationMismatch),
            }),
        }
    }
    async fn delete_unused(
        &mut self,
        id: ListingSourceId,
        expected: ListingSourceStorageVersion,
    ) -> Result<(), ListingSourceRepositoryError> {
        let id = uuid::Uuid::from(id);
        let expected = i64::try_from(expected.into_inner()).map_err(|error| {
            ListingSourceRepositoryError::InvalidPersistedState {
                source: box_error(error),
            }
        })?;
        // Cleanup is explicit, although these relationships also cascade as a schema backstop.
        sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id=$1")
            .bind(id)
            .execute(&mut *self.connection)
            .await
            .map_err(db_write)?;
        sqlx::query("DELETE FROM listing_source_web_crawl_ingestion_configurations WHERE listing_source_id=$1")
            .bind(id).execute(&mut *self.connection).await.map_err(db_write)?;
        sqlx::query("DELETE FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1")
            .bind(id).execute(&mut *self.connection).await.map_err(db_write)?;
        sqlx::query("DELETE FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id=$1")
            .bind(id).execute(&mut *self.connection).await.map_err(db_write)?;
        sqlx::query("DELETE FROM listing_source_ingestion_methods WHERE listing_source_id=$1")
            .bind(id)
            .execute(&mut *self.connection)
            .await
            .map_err(db_write)?;
        let result =
            sqlx::query("DELETE FROM listing_sources WHERE listing_source_id=$1 AND version=$2")
                .bind(id)
                .bind(expected)
                .execute(&mut *self.connection)
                .await
                .map_err(db_write)?;
        if result.rows_affected() != 1 {
            return Err(ListingSourceRepositoryError::ConcurrencyConflict);
        }
        Ok(())
    }
    async fn insert(
        &mut self,
        source: &ListingSource,
        configuration: &ListingSourceIngestionConfigurations,
        woocommerce_webhook_secret: Option<&str>,
    ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
        configuration.validate_for(source).map_err(|_| {
            ListingSourceRepositoryError::InvalidPersistedState {
                source: box_error(ListingIngestionConfigurationMismatch),
            }
        })?;
        let referral_configuration = referral_json(source.referral_configuration());
        let row=sqlx::query_as::<_,SourceRow>("INSERT INTO listing_sources (listing_source_id,listing_source_slug_id,name,operator_party_id,url,image,referral_configuration) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING listing_source_id,listing_source_slug_id,name,operator_party_id,url,image,referral_configuration,version,created,updated").bind(uuid::Uuid::from(source.id())).bind(source.slug_id().as_ref()).bind(source.name().as_ref()).bind(uuid::Uuid::from(source.operator_party_id())).bind(source.presentation().url.as_ref().map(Url::as_str)).bind(source.presentation().image.as_ref().map(Url::as_str)).bind(referral_configuration).fetch_one(&mut *self.connection).await.map_err(db_write)?;
        write_configuration(
            self.connection,
            source.id(),
            configuration,
            woocommerce_webhook_secret,
        )
        .await?;
        load(self.connection, row).await
    }
    async fn update(
        &mut self,
        source: &ListingSource,
        configuration: &ListingSourceIngestionConfigurations,
        woocommerce_webhook_secret: PatchField<&str>,
        expected: ListingSourceStorageVersion,
    ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
        configuration.validate_for(source).map_err(|_| {
            ListingSourceRepositoryError::InvalidPersistedState {
                source: box_error(ListingIngestionConfigurationMismatch),
            }
        })?;
        let expected = i64::try_from(expected.into_inner()).map_err(|error| {
            ListingSourceRepositoryError::InvalidPersistedState {
                source: box_error(error),
            }
        })?;
        let webhook_secret = match woocommerce_webhook_secret {
            PatchField::Unchanged => {
                existing_woocommerce_webhook_secret(self.connection, source.id()).await?
            }
            PatchField::Set(secret) => Some(secret.to_owned()),
            PatchField::Clear => None,
        };
        let referral_configuration = referral_json(source.referral_configuration());
        let row=sqlx::query_as::<_,SourceRow>("UPDATE listing_sources SET name=$1,operator_party_id=$2,url=$3,image=$4,referral_configuration=$5,version=version+1,updated=now() WHERE listing_source_id=$6 AND version=$7 RETURNING listing_source_id,listing_source_slug_id,name,operator_party_id,url,image,referral_configuration,version,created,updated").bind(source.name().as_ref()).bind(uuid::Uuid::from(source.operator_party_id())).bind(source.presentation().url.as_ref().map(Url::as_str)).bind(source.presentation().image.as_ref().map(Url::as_str)).bind(referral_configuration).bind(uuid::Uuid::from(source.id())).bind(expected).fetch_optional(&mut *self.connection).await.map_err(db_write)?.ok_or(ListingSourceRepositoryError::ConcurrencyConflict)?;
        sqlx::query("DELETE FROM listing_source_ingestion_methods WHERE listing_source_id=$1")
            .bind(uuid::Uuid::from(source.id()))
            .execute(&mut *self.connection)
            .await
            .map_err(db_write)?;
        sqlx::query(
            "DELETE FROM listing_source_web_crawl_ingestion_configurations WHERE listing_source_id=$1",
        )
        .bind(uuid::Uuid::from(source.id()))
        .execute(&mut *self.connection)
        .await
        .map_err(db_write)?;
        sqlx::query("DELETE FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1")
            .bind(uuid::Uuid::from(source.id()))
            .execute(&mut *self.connection)
            .await
            .map_err(db_write)?;
        sqlx::query(
            "DELETE FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id=$1",
        )
        .bind(uuid::Uuid::from(source.id()))
        .execute(&mut *self.connection)
        .await
        .map_err(db_write)?;
        write_configuration(
            self.connection,
            source.id(),
            configuration,
            webhook_secret.as_deref(),
        )
        .await?;
        load(self.connection, row).await
    }
}
async fn load(
    connection: &mut PgConnection,
    row: SourceRow,
) -> Result<StoredListingSource, ListingSourceRepositoryError> {
    let methods = sqlx::query_as::<_, MethodRow>(
        "SELECT ingestion_method FROM listing_source_ingestion_methods WHERE listing_source_id=$1",
    )
    .bind(row.listing_source_id)
    .fetch_all(&mut *connection)
    .await
    .map_err(db_read)?
    .into_iter()
    .map(|row| row.ingestion_method.parse())
    .collect::<Result<HashSet<ListingIngestionMethod>, _>>()
    .map_err(
        |error| ListingSourceRepositoryError::InvalidPersistedState {
            source: box_error(error),
        },
    )?;
    let config = read_configuration(connection, row.listing_source_id).await?;
    let source = ListingSource::rehydrate(RehydratedListingSourceState {
        id: ListingSourceId::from(row.listing_source_id),
        slug_id: row.listing_source_slug_id,
        name: row.name,
        operator_party_id: party_core::party_id::PartyId::from(row.operator_party_id),
        ingestion_methods: methods,
        presentation: ListingSourcePresentation {
            url: row
                .url
                .map(|value| Url::parse(&value))
                .transpose()
                .map_err(invalid)?,
            image: row
                .image
                .map(|value| Url::parse(&value))
                .transpose()
                .map_err(invalid)?,
        },
        referral_configuration: parse_referral(row.referral_configuration)?,
    })
    .map_err(invalid)?;
    config.validate_for(&source).map_err(invalid)?;
    let version = ListingSourceStorageVersion::try_from(row.version).map_err(invalid)?;
    Ok(StoredListingSource {
        source,
        configuration: config,
        version,
        created: row.created,
        updated: row.updated,
    })
}
async fn existing_woocommerce_webhook_secret(
    connection: &mut PgConnection,
    id: ListingSourceId,
) -> Result<Option<String>, ListingSourceRepositoryError> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT webhook_secret FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id=$1",
    )
    .bind(uuid::Uuid::from(id))
    .fetch_optional(&mut *connection)
    .await
    .map_err(db_read)
    .map(Option::flatten)
}

async fn write_configuration(
    connection: &mut PgConnection,
    id: ListingSourceId,
    configs: &ListingSourceIngestionConfigurations,
    woocommerce_webhook_secret: Option<&str>,
) -> Result<(), ListingSourceRepositoryError> {
    for config in &configs.0 {
        sqlx::query("INSERT INTO listing_source_ingestion_methods (listing_source_id,ingestion_method) VALUES ($1,$2)").bind(uuid::Uuid::from(id)).bind(config.method().as_str()).execute(&mut *connection).await.map_err(db_write)?;
        match config {
            ListingIngestionConfiguration::Shopify {
                domain,
                currency,
                language,
            } => {
                sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations (listing_source_id,domain,currency,language) VALUES ($1,$2,$3,$4)").bind(uuid::Uuid::from(id)).bind(domain.as_str()).bind(currency.map(|v|v.as_str())).bind(language.map(|v|v.as_str())).execute(&mut *connection).await.map_err(db_write)?;
            }
            ListingIngestionConfiguration::Woocommerce { currency, language } => {
                sqlx::query("INSERT INTO listing_source_woocommerce_ingestion_configurations (listing_source_id,webhook_secret,currency,language) VALUES ($1,$2,$3,$4)").bind(uuid::Uuid::from(id)).bind(woocommerce_webhook_secret).bind(currency.map(|v|v.as_str())).bind(language.map(|v|v.as_str())).execute(&mut *connection).await.map_err(db_write)?;
            }
            ListingIngestionConfiguration::WebCrawl { fallback_currency } => {
                sqlx::query("INSERT INTO listing_source_web_crawl_ingestion_configurations (listing_source_id,fallback_currency) VALUES ($1,$2)").bind(uuid::Uuid::from(id)).bind(fallback_currency.map(|v|v.as_str())).execute(&mut *connection).await.map_err(db_write)?;
            }
            ListingIngestionConfiguration::PartnerApi => {}
        }
    }
    Ok(())
}
async fn read_configuration(
    connection: &mut PgConnection,
    id: uuid::Uuid,
) -> Result<ListingSourceIngestionConfigurations, ListingSourceRepositoryError> {
    let methods = sqlx::query_as::<_, MethodRow>(
        "SELECT ingestion_method FROM listing_source_ingestion_methods WHERE listing_source_id=$1",
    )
    .bind(id)
    .fetch_all(&mut *connection)
    .await
    .map_err(db_read)?;
    let mut configs = Vec::new();
    for row in methods {
        match row.ingestion_method.parse().map_err(invalid)? {
            ListingIngestionMethod::WebCrawl => {
                let fallback_currency=sqlx::query_scalar::<_,Option<String>>("SELECT fallback_currency FROM listing_source_web_crawl_ingestion_configurations WHERE listing_source_id=$1").bind(id).fetch_optional(&mut *connection).await.map_err(db_read)?.ok_or_else(|| invalid(ListingIngestionConfigurationMismatch))?;
                configs.push(ListingIngestionConfiguration::WebCrawl {
                    fallback_currency: parse_optional_currency(fallback_currency.as_deref())?,
                });
            }
            ListingIngestionMethod::PartnerApi => {
                configs.push(ListingIngestionConfiguration::PartnerApi)
            }
            ListingIngestionMethod::Shopify => {
                let row=sqlx::query_as::<_,(String,Option<String>,Option<String>)>("SELECT domain,currency,language FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1").bind(id).fetch_optional(&mut *connection).await.map_err(db_read)?.ok_or_else(|| invalid(ListingIngestionConfigurationMismatch))?;
                configs.push(ListingIngestionConfiguration::Shopify {
                    domain: Domain::try_from(row.0).map_err(invalid)?,
                    currency: parse_optional_currency(row.1.as_deref())?,
                    language: parse_optional_language(row.2.as_deref())?,
                });
            }
            ListingIngestionMethod::Woocommerce => {
                let row=sqlx::query_as::<_,(Option<String>,Option<String>)>("SELECT currency,language FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id=$1").bind(id).fetch_optional(&mut *connection).await.map_err(db_read)?.ok_or_else(|| invalid(ListingIngestionConfigurationMismatch))?;
                configs.push(ListingIngestionConfiguration::Woocommerce {
                    currency: parse_optional_currency(row.0.as_deref())?,
                    language: parse_optional_language(row.1.as_deref())?,
                });
            }
        }
    }
    let methods = configs
        .iter()
        .map(ListingIngestionConfiguration::method)
        .collect::<HashSet<_>>();
    let has_orphan_web_crawl = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM listing_source_web_crawl_ingestion_configurations WHERE listing_source_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut *connection)
    .await
    .map_err(db_read)?
        && !methods.contains(&ListingIngestionMethod::WebCrawl);
    let has_orphan_shopify = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut *connection)
    .await
    .map_err(db_read)?
        && !methods.contains(&ListingIngestionMethod::Shopify);
    let has_orphan_woocommerce = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut *connection)
    .await
    .map_err(db_read)?
        && !methods.contains(&ListingIngestionMethod::Woocommerce);
    if has_orphan_web_crawl || has_orphan_shopify || has_orphan_woocommerce {
        return Err(invalid(ListingIngestionConfigurationMismatch));
    }
    let configurations = ListingSourceIngestionConfigurations(configs);
    configurations.methods().map_err(invalid)?;
    Ok(configurations)
}
fn parse_optional_currency(
    value: Option<&str>,
) -> Result<Option<money::Currency>, ListingSourceRepositoryError> {
    value
        .map(|currency| {
            money::Currency::from_code(currency)
                .ok_or_else(|| invalid(ListingIngestionConfigurationMismatch))
        })
        .transpose()
}

fn parse_optional_language(
    value: Option<&str>,
) -> Result<Option<localization::Language>, ListingSourceRepositoryError> {
    value
        .map(|language| {
            localization::Language::from_code(language)
                .ok_or_else(|| invalid(ListingIngestionConfigurationMismatch))
        })
        .transpose()
}

fn referral_json(value: Option<&ReferralConfiguration>) -> Option<serde_json::Value> {
    value.map(|value| match value {
        ReferralConfiguration::Partnerize { camref } => {
            serde_json::json!({"kind":"PARTNERIZE","camref":camref.as_ref()})
        }
    })
}
fn parse_referral(
    value: Option<serde_json::Value>,
) -> Result<Option<ReferralConfiguration>, ListingSourceRepositoryError> {
    value
        .map(|value| {
            let camref = value
                .get("camref")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid(ListingIngestionConfigurationMismatch))?;
            if value.get("kind").and_then(serde_json::Value::as_str) == Some("PARTNERIZE") {
                Ok(ReferralConfiguration::Partnerize {
                    camref: PartnerizeCamref::try_from(camref).map_err(invalid)?,
                })
            } else {
                Err(invalid(ListingIngestionConfigurationMismatch))
            }
        })
        .transpose()
}
fn invalid(error: impl std::error::Error + Send + Sync + 'static) -> ListingSourceRepositoryError {
    ListingSourceRepositoryError::InvalidPersistedState {
        source: box_error(error),
    }
}
fn db_read(error: sqlx::Error) -> ListingSourceRepositoryError {
    ListingSourceRepositoryError::TemporarilyUnavailable {
        source: box_error(error),
    }
}
fn db_write(error: sqlx::Error) -> ListingSourceRepositoryError {
    match &error {
        sqlx::Error::Database(database_error)
            if database_error.constraint() == Some("listing_sources_slug_unique") =>
        {
            ListingSourceRepositoryError::SlugConflict {
                source: box_error(error),
            }
        }
        sqlx::Error::Database(database_error)
            if database_error.constraint() == Some("listing_source_shopify_domain_unique") =>
        {
            ListingSourceRepositoryError::ShopifyDomainConflict {
                source: box_error(error),
            }
        }
        _ => ListingSourceRepositoryError::Internal {
            source: box_error(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SqlxListingSourceReaders, SqlxListingSourceRepositoryFactory};
    use application::transaction::{Transaction, UnitOfWork};
    use listing_source_service::ports::{
        ListingSourceDetailsReader, ListingSourceRepository, ListingSourceRepositoryFactory,
        ShopifySourceReader, WebCrawlSourceReader, WoocommerceSignatureVerifier,
        WoocommerceSourceReader,
    };
    use party_core::{party_id::PartyId, party_name::PartyName};
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    async fn insert_user(pool: &sqlx::PgPool) -> uuid::Uuid {
        let user_id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(user_id)
        .bind(format!("delete-test-{user_id}@example.test"))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert proposal applicant: {error}"));
        user_id
    }

    fn existing_source_proposal(source_id: ListingSourceId) -> serde_json::Value {
        serde_json::json!({
            "type": "EXISTING_LISTING_SOURCE",
            "listing_source_id": source_id.to_string(),
        })
    }

    async fn wait_until_backend_waits_for_lock(pool: &sqlx::PgPool, backend_pid: i32) {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let wait_event_type = sqlx::query_scalar::<_, String>(
                "SELECT COALESCE(wait_event_type, '') FROM pg_stat_activity WHERE pid = $1",
            )
            .bind(backend_pid)
            .fetch_optional(pool)
            .await
            .unwrap_or_else(|error| panic!("inspect concurrent backend state: {error}"));
            if wait_event_type.as_deref() == Some("Lock") {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("concurrent PostgreSQL backend did not wait for its row lock");
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn insert_basic_source(pool: &sqlx::PgPool, name: &str) -> (ListingSourceId, PartyId) {
        let source_id = ListingSourceId::new();
        let party_id = PartyId::new();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::from(party_id))
            .bind(format!("delete-test-party-{party_id}"))
            .bind(format!("{name} operator"))
            .execute(pool)
            .await
            .unwrap_or_else(|error| panic!("insert source operator: {error}"));
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(uuid::Uuid::from(source_id))
        .bind(format!("delete-test-source-{source_id}"))
        .bind(name)
        .bind(uuid::Uuid::from(party_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert listing source: {error}"));
        sqlx::query(
            "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'PARTNER_API')",
        )
        .bind(uuid::Uuid::from(source_id))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("insert source ingestion method: {error}"));
        (source_id, party_id)
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_persist_and_read_operator_provider_configuration_and_webcrawl_source() {
        let pool = get_postgres_client().await;
        let operator_party_id = PartyId::new();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::from(operator_party_id))
            .bind("operator")
            .bind("Operator")
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert operator party: {error}"));

        let mut source = ListingSource::create(NewListingSource {
            id: ListingSourceId::new(),
            name: ListingSourceName::try_from("Provider Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            operator_party_id,
            ingestion_methods: HashSet::from([
                ListingIngestionMethod::WebCrawl,
                ListingIngestionMethod::Shopify,
                ListingIngestionMethod::Woocommerce,
            ]),
            presentation: ListingSourcePresentation {
                url: Some(
                    Url::parse("https://provider.example")
                        .unwrap_or_else(|error| panic!("test URL: {error}")),
                ),
                image: None,
            },
            referral_configuration: None,
        });
        let configuration = ListingSourceIngestionConfigurations(vec![
            ListingIngestionConfiguration::WebCrawl {
                fallback_currency: Some(money::Currency::Eur),
            },
            ListingIngestionConfiguration::Shopify {
                domain: Domain::try_from("shop.provider.example")
                    .unwrap_or_else(|error| panic!("test domain: {error}")),
                currency: Some(money::Currency::Eur),
                language: Some(localization::Language::En),
            },
            ListingIngestionConfiguration::Woocommerce {
                currency: Some(money::Currency::Usd),
                language: Some(localization::Language::De),
            },
        ]);
        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool.clone());
        let mut transaction = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin transaction: {error}"));
        let stored = SqlxListingSourceRepositoryFactory::new()
            .in_transaction(&mut transaction)
            .insert(&source, &configuration, Some("test-webhook-secret"))
            .await
            .unwrap_or_else(|error| panic!("insert listing source: {error}"));
        transaction
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit transaction: {error}"));
        let partnership_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
            .bind(partnership_id)
            .bind(uuid::Uuid::from(operator_party_id))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert operator partnership: {error}"));
        sqlx::query(
            "INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)",
        )
        .bind(partnership_id)
        .bind(uuid::Uuid::from(source.id()))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert listing source grant: {error}"));

        assert_eq!(source.id(), stored.source.id());
        assert_eq!(configuration, stored.configuration);

        let readers = SqlxListingSourceReaders::new(pool.clone());
        let details = readers
            .find_details_by_id(source.id())
            .await
            .unwrap_or_else(|error| panic!("read listing source details: {error}"))
            .unwrap_or_else(|| panic!("listing source details missing"));
        assert_eq!(
            PartyName::try_from("Operator")
                .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
            details.operator_name
        );
        assert_eq!(operator_party_id, details.operator_party_id);
        assert_eq!(source.ingestion_methods(), &details.ingestion_methods);

        let shopify = readers
            .find_by_domain(
                &Domain::try_from("shop.provider.example")
                    .unwrap_or_else(|error| panic!("test domain: {error}")),
            )
            .await
            .unwrap_or_else(|error| panic!("read Shopify source: {error}"))
            .unwrap_or_else(|| panic!("Shopify source missing"));
        assert_eq!(Some(money::Currency::Eur), shopify.currency);
        assert_eq!(Some(localization::Language::En), shopify.language);

        let woocommerce = readers
            .find_by_id(source.id())
            .await
            .unwrap_or_else(|error| panic!("read WooCommerce source: {error}"))
            .unwrap_or_else(|| panic!("WooCommerce source missing"));
        assert_eq!(Some(money::Currency::Usd), woocommerce.currency);
        assert_eq!(Some(localization::Language::De), woocommerce.language);

        let webcrawl = readers
            .list_sources()
            .await
            .unwrap_or_else(|error| panic!("list webcrawl sources: {error}"));
        assert!(webcrawl.iter().any(|candidate| {
            candidate.listing_source_id == source.id()
                && candidate.listing_source_name == *source.name()
                && candidate.listing_source_slug == *source.slug_id()
                && candidate.fallback_currency == Some(money::Currency::Eur)
        }));

        let body = b"payload";
        let signature = hmac_sha256(b"test-webhook-secret", body)
            .unwrap_or_else(|error| panic!("sign webhook: {error}"));
        assert_eq!(
            WoocommerceSignatureVerification::Valid,
            readers
                .verify(source.id(), body, &signature)
                .await
                .unwrap_or_else(|error| panic!("verify WooCommerce signature: {error}"))
        );

        source.replace_ingestion_methods(HashSet::from([ListingIngestionMethod::PartnerApi]));
        let updated_configuration =
            ListingSourceIngestionConfigurations(vec![ListingIngestionConfiguration::PartnerApi]);
        let mut transaction = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin update transaction: {error}"));
        let updated = SqlxListingSourceRepositoryFactory::new()
            .in_transaction(&mut transaction)
            .update(
                &source,
                &updated_configuration,
                PatchField::Unchanged,
                stored.version,
            )
            .await
            .unwrap_or_else(|error| panic!("update listing source: {error}"));
        transaction
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit update transaction: {error}"));
        assert_eq!(updated_configuration, updated.configuration);
        let web_crawl_configured = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_source_web_crawl_ingestion_configurations WHERE listing_source_id=$1)",
        )
        .bind(uuid::Uuid::from(source.id()))
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("check deleted WebCrawl configuration: {error}"));
        assert!(!web_crawl_configured);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_explicitly_remove_source_configuration_and_grants_when_deleting_unused_source()
    {
        let pool = get_postgres_client().await;
        let (source_id, party_id) = insert_basic_source(&pool, "Delete target").await;
        let (other_source_id, other_party_id) =
            insert_basic_source(&pool, "Unrelated source").await;
        sqlx::query("INSERT INTO listing_source_web_crawl_ingestion_configurations (listing_source_id, fallback_currency) VALUES ($1, 'EUR')")
            .bind(uuid::Uuid::from(source_id))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert WebCrawl configuration: {error}"));
        sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations (listing_source_id, domain) VALUES ($1, 'delete-target.example')")
            .bind(uuid::Uuid::from(source_id))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert Shopify configuration: {error}"));
        sqlx::query("INSERT INTO listing_source_woocommerce_ingestion_configurations (listing_source_id, webhook_secret) VALUES ($1, 'nonempty-secret')")
            .bind(uuid::Uuid::from(source_id))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert WooCommerce configuration: {error}"));
        for method in ["WEB_CRAWL", "SHOPIFY", "WOOCOMMERCE"] {
            sqlx::query(
                "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, $2)",
            )
            .bind(uuid::Uuid::from(source_id))
            .bind(method)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert configured ingestion method: {error}"));
        }
        sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
            .bind(uuid::Uuid::new_v4())
            .bind(uuid::Uuid::from(party_id))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert target partnership: {error}"));
        let grant_partnership_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::new_v4())
            .bind(format!("delete-test-grant-party-{source_id}"))
            .bind("Grant party")
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert grant party: {error}"));
        let grant_party_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT party_id FROM parties WHERE party_slug_id = $1",
        )
        .bind(format!("delete-test-grant-party-{source_id}"))
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("find grant party: {error}"));
        sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
            .bind(grant_partnership_id)
            .bind(grant_party_id)
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert grant partnership: {error}"));
        sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)")
            .bind(grant_partnership_id)
            .bind(uuid::Uuid::from(source_id))
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert target grant: {error}"));

        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool.clone());
        let mut transaction = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin delete transaction: {error}"));
        let repository_factory = SqlxListingSourceRepositoryFactory::new();
        let mut repository = repository_factory.in_transaction(&mut transaction);
        let stored = repository
            .find_by_id_for_update(source_id)
            .await
            .unwrap_or_else(|error| panic!("lock listing source: {error}"))
            .unwrap_or_else(|| panic!("listing source missing before delete"));
        assert_eq!(
            None,
            repository
                .find_deletion_blocker(source_id)
                .await
                .unwrap_or_else(|error| panic!("check delete dependencies: {error}"))
        );
        repository
            .delete_unused(source_id, stored.version)
            .await
            .unwrap_or_else(|error| panic!("delete unused listing source: {error}"));
        drop(repository);
        transaction
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit source deletion: {error}"));

        for query in [
            "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
            "SELECT EXISTS (SELECT 1 FROM listing_source_ingestion_methods WHERE listing_source_id = $1)",
            "SELECT EXISTS (SELECT 1 FROM listing_source_web_crawl_ingestion_configurations WHERE listing_source_id = $1)",
            "SELECT EXISTS (SELECT 1 FROM listing_source_shopify_ingestion_configurations WHERE listing_source_id = $1)",
            "SELECT EXISTS (SELECT 1 FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id = $1)",
            "SELECT EXISTS (SELECT 1 FROM partnership_listing_source_grants WHERE listing_source_id = $1)",
        ] {
            let exists = sqlx::query_scalar::<_, bool>(query)
                .bind(uuid::Uuid::from(source_id))
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|error| panic!("check deleted source relation: {error}"));
            assert!(!exists, "deleted source relation remains");
        }
        let preserved = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1 AND operator_party_id = $2)",
        )
        .bind(uuid::Uuid::from(other_source_id))
        .bind(uuid::Uuid::from(other_party_id))
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("check unrelated source: {error}"));
        assert!(preserved);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_restrict_direct_source_delete_when_raw_stream_or_product_listing_exists() {
        let pool = get_postgres_client().await;
        let (raw_source_id, _) = insert_basic_source(&pool, "Raw blocker").await;
        sqlx::query("INSERT INTO product_listing_raw_streams (product_listing_raw_stream_id, listing_source_id, ingestion_method, source_record_key, source_record_key_sha256, latest_revision) VALUES ($1, $2, 'WEB_CRAWL', 'raw-only', $3, 0)")
            .bind(uuid::Uuid::new_v4())
            .bind(uuid::Uuid::from(raw_source_id))
            .bind(vec![1_u8; 32])
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert raw stream: {error}"));
        let raw_error = sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
            .bind(uuid::Uuid::from(raw_source_id))
            .execute(&pool)
            .await
            .expect_err("raw-stream source delete must be restricted");
        assert!(matches!(
            raw_error,
            sqlx::Error::Database(ref error) if error.constraint() == Some("product_listing_raw_streams_listing_source_id_fkey")
        ));

        let (product_source_id, _) = insert_basic_source(&pool, "Product blocker").await;
        let product_listing_id = uuid::Uuid::new_v4();
        let event_id = uuid::Uuid::new_v4();
        let mut transaction = pool
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin product-listing seed transaction: {error}"));
        sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, lifecycle, url) VALUES ($1, $2, $3, $3, $3, $4, 'withdrawn-product', 'WITHDRAWN', 'https://delete-test.example/product')")
            .bind(product_listing_id)
            .bind(format!("withdrawn-product-{}", &product_listing_id.simple().to_string()[..6]))
            .bind(event_id)
            .bind(uuid::Uuid::from(product_source_id))
            .execute(&mut *transaction)
            .await
            .unwrap_or_else(|error| panic!("insert withdrawn product listing: {error}"));
        sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
            .bind(event_id)
            .bind(product_listing_id)
            .bind(serde_json::json!({
                "title": null,
                "description": null,
                "listingSourceId": product_source_id.to_string(),
                "sourceListingId": "withdrawn-product",
                "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
                "availability": null,
                "url": "https://delete-test.example/product",
                "imageCount": 0,
                "auction": {"start": null, "end": null}
            }))
            .execute(&mut *transaction)
            .await
            .unwrap_or_else(|error| panic!("insert withdrawn product event: {error}"));
        transaction
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit product-listing seed: {error}"));
        let product_error = sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
            .bind(uuid::Uuid::from(product_source_id))
            .execute(&pool)
            .await
            .expect_err("product-listing source delete must be restricted");
        assert!(matches!(
            product_error,
            sqlx::Error::Database(ref error) if error.constraint() == Some("product_listings_listing_source_id_fkey")
        ));
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_reject_delayed_existing_source_proposal_after_source_delete_commits() {
        let pool = get_postgres_client().await;
        let (source_id, _) = insert_basic_source(&pool, "Delete race target").await;
        let applicant_user_id = insert_user(&pool).await;

        let mut delete_tx = pool
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin delete race transaction: {error}"));
        sqlx::query("SELECT 1 FROM listing_sources WHERE listing_source_id = $1 FOR UPDATE")
            .bind(uuid::Uuid::from(source_id))
            .execute(&mut *delete_tx)
            .await
            .unwrap_or_else(|error| panic!("lock delete race target: {error}"));

        let (proposal_pid_tx, proposal_pid_rx) = tokio::sync::oneshot::channel();
        let proposal_pool = pool.clone();
        let proposal = existing_source_proposal(source_id);
        let proposal_task = tokio::spawn(async move {
            let mut tx = proposal_pool.begin().await?;
            let backend_pid = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await?;
            let _ = proposal_pid_tx.send(backend_pid);
            sqlx::query(
                "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal) VALUES ($1, $2, 'SUBMITTED', $3)",
            )
            .bind(uuid::Uuid::new_v4())
            .bind(applicant_user_id)
            .bind(proposal)
            .execute(&mut *tx)
            .await?;
            tx.commit().await
        });
        let proposal_pid = proposal_pid_rx
            .await
            .unwrap_or_else(|_| panic!("proposal transaction did not expose its backend"));
        wait_until_backend_waits_for_lock(&pool, proposal_pid).await;

        sqlx::query("DELETE FROM listing_sources WHERE listing_source_id = $1")
            .bind(uuid::Uuid::from(source_id))
            .execute(&mut *delete_tx)
            .await
            .unwrap_or_else(|error| panic!("delete locked source: {error}"));
        delete_tx
            .commit()
            .await
            .unwrap_or_else(|error| panic!("commit source deletion: {error}"));

        let proposal_error = proposal_task
            .await
            .unwrap_or_else(|error| panic!("join delayed proposal task: {error}"))
            .expect_err("proposal must fail after committed source deletion");
        assert!(matches!(
            proposal_error,
            sqlx::Error::Database(ref error)
                if error.code().as_deref() == Some("23503")
                    && error.constraint() == Some("partnership_applications_existing_listing_source_id_fkey")
        ));
        let application_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM partnership_applications WHERE applicant_user_id = $1)",
        )
        .bind(applicant_user_id)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("check delayed proposal persistence: {error}"));
        assert!(!application_exists);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_observe_existing_source_proposal_after_its_key_share_lock_commits() {
        let pool = get_postgres_client().await;
        let (source_id, _) = insert_basic_source(&pool, "Proposal race target").await;
        let applicant_user_id = insert_user(&pool).await;
        let (proposal_ready_tx, proposal_ready_rx) = tokio::sync::oneshot::channel();
        let (commit_proposal_tx, commit_proposal_rx) = tokio::sync::oneshot::channel();
        let proposal_pool = pool.clone();
        let proposal = existing_source_proposal(source_id);
        let proposal_task = tokio::spawn(async move {
            let mut tx = proposal_pool.begin().await?;
            sqlx::query(
                "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal) VALUES ($1, $2, 'SUBMITTED', $3)",
            )
            .bind(uuid::Uuid::new_v4())
            .bind(applicant_user_id)
            .bind(proposal)
            .execute(&mut *tx)
            .await?;
            let _ = proposal_ready_tx.send(());
            let _ = commit_proposal_rx.await;
            tx.commit().await
        });
        proposal_ready_rx
            .await
            .unwrap_or_else(|_| panic!("proposal transaction did not acquire key-share lock"));

        let (delete_pid_tx, delete_pid_rx) = tokio::sync::oneshot::channel();
        let delete_pool = pool.clone();
        let delete_task = tokio::spawn(async move {
            let mut tx = delete_pool.begin().await?;
            let backend_pid = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await?;
            let _ = delete_pid_tx.send(backend_pid);
            sqlx::query("SELECT 1 FROM listing_sources WHERE listing_source_id = $1 FOR UPDATE")
                .bind(uuid::Uuid::from(source_id))
                .execute(&mut *tx)
                .await?;
            let blocker = SqlxListingSourceRepository {
                connection: &mut tx,
            }
            .find_deletion_blocker(source_id)
            .await
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
            Ok::<_, sqlx::Error>(blocker)
        });
        let delete_pid = delete_pid_rx
            .await
            .unwrap_or_else(|_| panic!("delete transaction did not expose its backend"));
        wait_until_backend_waits_for_lock(&pool, delete_pid).await;
        let _ = commit_proposal_tx.send(());

        proposal_task
            .await
            .unwrap_or_else(|error| panic!("join proposal transaction: {error}"))
            .unwrap_or_else(|error| panic!("commit proposal transaction: {error}"));
        let blocker = delete_task
            .await
            .unwrap_or_else(|error| panic!("join delete transaction: {error}"))
            .unwrap_or_else(|error| panic!("read delete blocker after proposal commit: {error}"));
        assert_eq!(
            Some(ListingSourceDeletionBlocker::ExistingSourcePartnershipApplication),
            blocker
        );
        let source_exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM listing_sources WHERE listing_source_id = $1)",
        )
        .bind(uuid::Uuid::from(source_id))
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("check proposal-won source: {error}"));
        assert!(source_exists);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_reject_orphan_web_crawl_configuration() {
        let pool = get_postgres_client().await;
        let operator_party_id = PartyId::new();
        let source_id = ListingSourceId::new();
        sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::from(operator_party_id))
            .bind(format!("orphan-web-crawl-operator-{operator_party_id}"))
            .bind("Orphan WebCrawl operator")
            .execute(&pool)
            .await
            .unwrap_or_else(|error| panic!("insert operator party: {error}"));
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(uuid::Uuid::from(source_id))
        .bind(format!("orphan-web-crawl-source-{source_id}"))
        .bind("Orphan WebCrawl source")
        .bind(uuid::Uuid::from(operator_party_id))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert listing source: {error}"));
        sqlx::query(
            "INSERT INTO listing_source_web_crawl_ingestion_configurations (listing_source_id) VALUES ($1)",
        )
        .bind(uuid::Uuid::from(source_id))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert orphan WebCrawl configuration: {error}"));

        let unit_of_work = platform_postgres::SqlxUnitOfWork::new(pool);
        let mut transaction = unit_of_work
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin transaction: {error}"));
        let result = SqlxListingSourceRepositoryFactory::new()
            .in_transaction(&mut transaction)
            .find_by_id(source_id)
            .await;

        assert!(matches!(
            result,
            Err(ListingSourceRepositoryError::InvalidPersistedState { .. })
        ));
    }

    fn hmac_sha256(secret: &[u8], body: &[u8]) -> Result<Vec<u8>, openssl::error::ErrorStack> {
        use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
        let key = PKey::hmac(secret)?;
        let mut signer = Signer::new(MessageDigest::sha256(), &key)?;
        signer.update(body)?;
        signer.sign_to_vec()
    }

    #[test]
    fn should_reject_unknown_or_noncanonical_persisted_currency_and_language() {
        assert!(parse_optional_currency(Some("INVALID")).is_err());
        assert!(parse_optional_currency(Some("eur")).is_err());
        assert!(parse_optional_language(Some("INVALID")).is_err());
    }

    #[test]
    fn should_reject_unsafe_partnerize_camref_from_persisted_configuration() {
        for camref in [" campaign", "campaign/ref", "campaign?ref", "café"] {
            let persisted = Some(serde_json::json!({"kind":"PARTNERIZE","camref":camref}));

            assert!(parse_referral(persisted).is_err(), "{camref:?}");
        }
    }

    #[test]
    fn should_serialize_valid_partnerize_camref() {
        let camref = PartnerizeCamref::try_from("1101l3AbC")
            .unwrap_or_else(|error| panic!("test camref: {error}"));
        let configuration = ReferralConfiguration::Partnerize { camref };

        assert_eq!(
            Some(serde_json::json!({"kind":"PARTNERIZE","camref":"1101l3AbC"})),
            referral_json(Some(&configuration))
        );
    }
}

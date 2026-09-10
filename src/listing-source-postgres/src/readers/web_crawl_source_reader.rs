use super::{SqlxListingSourceReaders, invalid_read, read_error};
use application::error::box_error;
use domain_primitives::object_id::ObjectIdError;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use listing_source_service::ports::{
    ListingIngestionConfigurationMismatch, ListingSourceReadError, WebCrawlSource,
    WebCrawlSourceReader,
};
use money::Currency;

#[derive(sqlx::FromRow)]
struct WebCrawlSourceRow {
    listing_source_id: uuid::Uuid,
    name: String,
    listing_source_slug_id: String,
    web_crawl_enabled: bool,
    web_crawl_configured: bool,
    fallback_currency: Option<String>,
}

#[async_trait::async_trait]
impl WebCrawlSourceReader for SqlxListingSourceReaders {
    async fn list_sources(&self) -> Result<Vec<WebCrawlSource>, ListingSourceReadError> {
        let rows = sqlx::query_as::<_, WebCrawlSourceRow>(
            "SELECT s.listing_source_id, s.name, s.listing_source_slug_id, \
                    EXISTS ( \
                        SELECT 1 FROM listing_source_ingestion_methods m \
                        WHERE m.listing_source_id = s.listing_source_id \
                          AND m.ingestion_method = 'WEB_CRAWL' \
                    ) AS web_crawl_enabled, \
                    c.listing_source_id IS NOT NULL AS web_crawl_configured, \
                    c.fallback_currency \
             FROM listing_sources s \
             LEFT JOIN listing_source_web_crawl_ingestion_configurations c \
               ON c.listing_source_id = s.listing_source_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(read_error)?;

        rows.into_iter().map(map_web_crawl_source).collect()
    }
}

fn map_web_crawl_source(row: WebCrawlSourceRow) -> Result<WebCrawlSource, ListingSourceReadError> {
    let fallback_currency = match (row.web_crawl_enabled, row.web_crawl_configured) {
        (true, true) => parse_optional_currency(row.fallback_currency.as_deref())?,
        (false, false) => None,
        _ => return Err(invalid_read(ListingIngestionConfigurationMismatch)),
    };

    Ok(WebCrawlSource {
        listing_source_id: ListingSourceId::try_from(row.listing_source_id)
            .map_err(InvalidWebCrawlSourceId)
            .map_err(invalid_read)?,
        listing_source_name: ListingSourceName::try_from(row.name).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?,
        listing_source_slug: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(
            |error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            },
        )?,
        web_crawl_enabled: row.web_crawl_enabled,
        fallback_currency,
    })
}

#[derive(Debug, thiserror::Error)]
#[error("invalid ListingSource ID persisted")]
struct InvalidWebCrawlSourceId(#[source] ObjectIdError);

fn parse_optional_currency(
    value: Option<&str>,
) -> Result<Option<Currency>, ListingSourceReadError> {
    value
        .map(|currency| {
            Currency::from_code(currency)
                .ok_or_else(|| invalid_read(ListingIngestionConfigurationMismatch))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        web_crawl_enabled: bool,
        web_crawl_configured: bool,
        fallback_currency: Option<&str>,
    ) -> WebCrawlSourceRow {
        WebCrawlSourceRow {
            listing_source_id: uuid::Uuid::now_v7(),
            name: "Source".into(),
            listing_source_slug_id: "source".into(),
            web_crawl_enabled,
            web_crawl_configured,
            fallback_currency: fallback_currency.map(str::to_owned),
        }
    }

    #[test]
    fn should_reject_wrong_version_listing_source_id() {
        let mut row = row(true, true, Some("EUR"));
        row.listing_source_id = uuid::Uuid::new_v4();

        assert!(map_web_crawl_source(row).is_err());
    }

    #[test]
    fn should_map_canonical_web_crawl_fallback_currency() {
        let source = map_web_crawl_source(row(true, true, Some("EUR")));

        assert!(matches!(
            source,
            Ok(WebCrawlSource {
                fallback_currency: Some(Currency::Eur),
                ..
            })
        ));
    }

    #[test]
    fn should_reject_noncanonical_or_unknown_web_crawl_fallback_currency() {
        for currency in ["eur", "INVALID"] {
            assert!(map_web_crawl_source(row(true, true, Some(currency))).is_err());
        }
    }

    #[test]
    fn should_reject_web_crawl_method_configuration_mismatch() {
        assert!(map_web_crawl_source(row(true, false, None)).is_err());
        assert!(map_web_crawl_source(row(false, true, None)).is_err());
    }
}

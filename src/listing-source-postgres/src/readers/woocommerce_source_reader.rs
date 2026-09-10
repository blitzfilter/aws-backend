use super::{SqlxListingSourceReaders, invalid_read, read_error};
use domain_primitives::object_id::ObjectIdError;
use listing_source_core::ListingSourceId;
use listing_source_service::ports::{
    ListingSourceReadError, WoocommerceSource, WoocommerceSourceReader,
};
use localization::Language;
use money::Currency;

#[derive(sqlx::FromRow)]
struct WooRow {
    listing_source_id: uuid::Uuid,
    currency: Option<String>,
    language: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("invalid ListingSource ID persisted")]
struct InvalidWoocommerceSourceId(#[source] ObjectIdError);

impl TryFrom<WooRow> for WoocommerceSource {
    type Error = ListingSourceReadError;

    fn try_from(row: WooRow) -> Result<Self, Self::Error> {
        Ok(Self {
            listing_source_id: ListingSourceId::try_from(row.listing_source_id)
                .map_err(InvalidWoocommerceSourceId)
                .map_err(invalid_read)?,
            currency: parse_optional_currency(row.currency.as_deref())?,
            language: parse_optional_language(row.language.as_deref())?,
        })
    }
}

fn parse_optional_currency(
    value: Option<&str>,
) -> Result<Option<Currency>, ListingSourceReadError> {
    value
        .map(|currency| {
            Currency::from_code(currency).ok_or_else(|| {
                invalid_read(std::io::Error::other(
                    "persisted listing source currency is invalid",
                ))
            })
        })
        .transpose()
}

fn parse_optional_language(
    value: Option<&str>,
) -> Result<Option<Language>, ListingSourceReadError> {
    value
        .map(|language| {
            Language::from_code(language).ok_or_else(|| {
                invalid_read(std::io::Error::other(
                    "persisted listing source language is invalid",
                ))
            })
        })
        .transpose()
}

#[async_trait::async_trait]
impl WoocommerceSourceReader for SqlxListingSourceReaders {
    async fn find_by_id(
        &self,
        id: ListingSourceId,
    ) -> Result<Option<WoocommerceSource>, ListingSourceReadError> {
        sqlx::query_as::<_, WooRow>(
            "SELECT c.listing_source_id,c.currency,c.language \
             FROM listing_source_woocommerce_ingestion_configurations c \
             JOIN listing_sources s ON s.listing_source_id=c.listing_source_id \
             JOIN listing_source_ingestion_methods m \
               ON m.listing_source_id=c.listing_source_id AND m.ingestion_method='WOOCOMMERCE' \
             JOIN partnerships p ON p.party_id=s.operator_party_id \
             WHERE c.listing_source_id=$1 \
               AND EXISTS ( \
                   SELECT 1 \
                   FROM partnership_listing_source_grants source_grant \
                   WHERE source_grant.partnership_id=p.partnership_id \
                     AND source_grant.listing_source_id=c.listing_source_id \
               )",
        )
        .bind(id.into_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(read_error)?
        .map(WoocommerceSource::try_from)
        .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(listing_source_id: uuid::Uuid) -> WooRow {
        WooRow {
            listing_source_id,
            currency: Some("EUR".to_owned()),
            language: Some("en".to_owned()),
        }
    }

    #[test]
    fn should_map_valid_uuidv7_listing_source_id() {
        assert!(WoocommerceSource::try_from(row(uuid::Uuid::now_v7())).is_ok());
    }

    #[test]
    fn should_reject_wrong_version_listing_source_id() {
        assert!(WoocommerceSource::try_from(row(uuid::Uuid::new_v4())).is_err());
    }
}

use application::error::box_error;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use partnership_service::ports::*;
use sqlx::PgPool;
use user_core::user_id::UserId;
#[derive(Debug, thiserror::Error)]
#[error("invalid administered ListingSource ID persisted")]
struct InvalidAdministeredListingSourceId(#[source] domain_primitives::object_id::ObjectIdError);

#[derive(sqlx::FromRow)]
struct AdministeredListingSourceRow {
    listing_source_id: uuid::Uuid,
    listing_source_slug_id: String,
    name: String,
}

impl TryFrom<AdministeredListingSourceRow> for AdministeredListingSource {
    type Error = SourceAuthorizationError;

    fn try_from(row: AdministeredListingSourceRow) -> Result<Self, Self::Error> {
        Ok(Self {
            listing_source_id: ListingSourceId::try_from(row.listing_source_id).map_err(
                |error| SourceAuthorizationError::InvalidReadModel {
                    source: box_error(InvalidAdministeredListingSourceId(error)),
                },
            )?,
            slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(|error| {
                SourceAuthorizationError::InvalidReadModel {
                    source: box_error(error),
                }
            })?,
            name: ListingSourceName::try_from(row.name).map_err(|error| {
                SourceAuthorizationError::InvalidReadModel {
                    source: box_error(error),
                }
            })?,
        })
    }
}

#[derive(Clone)]
pub struct SqlxListingSourceAuthorization {
    pool: PgPool,
}
impl SqlxListingSourceAuthorization {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
#[async_trait::async_trait]
impl ListingSourceAuthorization for SqlxListingSourceAuthorization {
    async fn can_write_source(
        &self,
        user_id: UserId,
        listing_source_id: ListingSourceId,
    ) -> Result<bool, SourceAuthorizationError> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(\
                SELECT 1 \
                FROM partnership_members member \
                JOIN partnership_listing_source_grants source_grant \
                  ON source_grant.partnership_id = member.partnership_id \
                JOIN partnerships partnership \
                  ON partnership.partnership_id = source_grant.partnership_id \
                JOIN listing_sources source \
                  ON source.listing_source_id = source_grant.listing_source_id \
                WHERE member.user_id = $1 \
                  AND source_grant.listing_source_id = $2 \
                  AND partnership.party_id = source.operator_party_id \
                  AND partnership.business_state = 'ACTIVE'\
            )",
        )
        .bind(user_id.into_uuid())
        .bind(listing_source_id.into_uuid())
        .fetch_one(&self.pool)
        .await
        .map_err(|source| SourceAuthorizationError::TemporarilyUnavailable {
            source: box_error(source),
        })
    }
    async fn list_sources_user_administers(
        &self,
        user_id: UserId,
    ) -> Result<Vec<AdministeredListingSource>, SourceAuthorizationError> {
        let rows = sqlx::query_as::<_, AdministeredListingSourceRow>(
            "SELECT DISTINCT s.listing_source_id, s.listing_source_slug_id, s.name \
             FROM partnership_members member \
             JOIN partnership_listing_source_grants source_grant \
               ON source_grant.partnership_id = member.partnership_id \
             JOIN listing_sources s \
               ON s.listing_source_id = source_grant.listing_source_id \
             JOIN partnerships partnership \
               ON partnership.partnership_id = source_grant.partnership_id \
              AND partnership.party_id = s.operator_party_id \
              AND partnership.business_state = 'ACTIVE' \
             WHERE member.user_id = $1 \
             ORDER BY s.name",
        )
        .bind(user_id.into_uuid())
        .fetch_all(&self.pool)
        .await
        .map_err(|source| SourceAuthorizationError::TemporarilyUnavailable {
            source: box_error(source),
        })?;
        rows.into_iter()
            .map(AdministeredListingSource::try_from)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(listing_source_id: uuid::Uuid) -> AdministeredListingSourceRow {
        AdministeredListingSourceRow {
            listing_source_id,
            listing_source_slug_id: "source".to_owned(),
            name: "Source".to_owned(),
        }
    }

    #[test]
    fn should_map_valid_uuidv7_listing_source_id() {
        assert!(AdministeredListingSource::try_from(row(uuid::Uuid::now_v7())).is_ok());
    }

    #[test]
    fn should_reject_wrong_version_listing_source_id() {
        assert!(AdministeredListingSource::try_from(row(uuid::Uuid::new_v4())).is_err());
    }
}

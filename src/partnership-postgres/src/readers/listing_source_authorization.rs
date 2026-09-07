use application::error::box_error;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use partnership_service::ports::*;
use sqlx::PgPool;
use user_core::user_id::UserId;
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
        .bind(uuid::Uuid::from(user_id))
        .bind(uuid::Uuid::from(listing_source_id))
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
        #[derive(sqlx::FromRow)]
        struct Row {
            listing_source_id: uuid::Uuid,
            listing_source_slug_id: String,
            name: String,
        }
        let rows = sqlx::query_as::<_, Row>(
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
        .bind(uuid::Uuid::from(user_id))
        .fetch_all(&self.pool)
        .await
        .map_err(|source| SourceAuthorizationError::TemporarilyUnavailable {
            source: box_error(source),
        })?;
        rows.into_iter()
            .map(|row| {
                Ok(AdministeredListingSource {
                    listing_source_id: ListingSourceId::from(row.listing_source_id),
                    slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(|e| {
                        SourceAuthorizationError::InvalidReadModel {
                            source: box_error(e),
                        }
                    })?,
                    name: ListingSourceName::try_from(row.name).map_err(|error| {
                        SourceAuthorizationError::InvalidReadModel {
                            source: box_error(error),
                        }
                    })?,
                })
            })
            .collect()
    }
}

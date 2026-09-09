use crate::object_id::try_from_uuid;
use application::error::box_error;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingCurrentEventCheck, ProductListingCurrentEventCheckError,
    ProductListingCurrentEventGuard, ProductListingCurrentEventGuardFactory,
    ProductListingCurrentEventRef,
};
use sqlx::PgConnection;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingCurrentEventGuardFactory;

struct SqlxProductListingCurrentEventGuard<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product current event guard SQL query failed")]
struct ProductListingCurrentEventGuardSqlxError(#[source] sqlx::Error);

impl SqlxProductListingCurrentEventGuardFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingCurrentEventGuardFactory<SqlxTransaction>
    for SqlxProductListingCurrentEventGuardFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingCurrentEventGuard + 'tx {
        SqlxProductListingCurrentEventGuard {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingCurrentEventGuard for SqlxProductListingCurrentEventGuard<'_> {
    async fn lock_and_check(
        &mut self,
        product_listing_id: ProductListingId,
        expected_event_id: EventId,
    ) -> Result<ProductListingCurrentEventCheck, ProductListingCurrentEventCheckError> {
        let current_event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT current_event_id FROM product_listings WHERE product_listing_id = $1 FOR SHARE",
        )
        .bind(product_listing_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| ProductListingCurrentEventCheckError::CheckFailed {
            source: box_error(ProductListingCurrentEventGuardSqlxError(source)),
        })?;

        Ok(match current_event_id {
            Some(event_id)
                if try_from_uuid::<EventId>(event_id, "current event ID").map_err(
                    |source| ProductListingCurrentEventCheckError::CheckFailed {
                        source: box_error(source),
                    },
                )? == expected_event_id =>
            {
                ProductListingCurrentEventCheck::Current
            }
            Some(_) | None => ProductListingCurrentEventCheck::Stale,
        })
    }

    async fn lock_and_check_all(
        &mut self,
        refs: &[ProductListingCurrentEventRef],
    ) -> Result<
        HashMap<ProductListingCurrentEventRef, ProductListingCurrentEventCheck>,
        ProductListingCurrentEventCheckError,
    > {
        if refs.is_empty() {
            return Ok(HashMap::new());
        }

        let product_listing_ids = refs
            .iter()
            .map(|reference| reference.product_listing_id.into_uuid())
            .collect::<Vec<_>>();
        let current_event_ids = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid)>(
            r#"
            SELECT product_listing_id, current_event_id
            FROM product_listings
            WHERE product_listing_id = ANY($1::uuid[])
            FOR SHARE
            "#,
        )
        .bind(product_listing_ids)
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|source| ProductListingCurrentEventCheckError::CheckFailed {
            source: box_error(ProductListingCurrentEventGuardSqlxError(source)),
        })?
        .into_iter()
        .map(|(product_listing_id, current_event_id)| {
            let product_listing_id =
                try_from_uuid::<ProductListingId>(product_listing_id, "ProductListing ID")
                    .map_err(|source| ProductListingCurrentEventCheckError::CheckFailed {
                        source: box_error(source),
                    })?;
            let current_event_id = try_from_uuid::<EventId>(current_event_id, "current event ID")
                .map_err(|source| {
                ProductListingCurrentEventCheckError::CheckFailed {
                    source: box_error(source),
                }
            })?;
            Ok((product_listing_id, current_event_id))
        })
        .collect::<Result<HashMap<_, _>, ProductListingCurrentEventCheckError>>()?;

        Ok(refs
            .iter()
            .copied()
            .map(|reference| {
                let check = match current_event_ids.get(&reference.product_listing_id) {
                    Some(event_id) if *event_id == reference.expected_event_id => {
                        ProductListingCurrentEventCheck::Current
                    }
                    Some(_) | None => ProductListingCurrentEventCheck::Stale,
                };
                (reference, check)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error = ProductListingCurrentEventCheckError::CheckFailed {
            source: box_error(ProductListingCurrentEventGuardSqlxError(
                sqlx::Error::RowNotFound,
            )),
        };

        let ProductListingCurrentEventCheckError::CheckFailed { source } = error;
        assert!(
            source
                .downcast_ref::<ProductListingCurrentEventGuardSqlxError>()
                .is_some()
        );
    }
}

use application::error::box_error;
use platform_postgres::SqlxTransaction;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use product_listing_service::ports::{
    ProductListingLifecycleGuard, ProductListingLifecycleGuardError,
    ProductListingLifecycleGuardFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingLifecycleGuardFactory;

struct SqlxProductListingLifecycleGuard<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, thiserror::Error)]
#[error("product listing lifecycle guard SQL query failed")]
struct ProductListingLifecycleGuardSqlxError(#[source] sqlx::Error);

impl SqlxProductListingLifecycleGuardFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingLifecycleGuardFactory<SqlxTransaction>
    for SqlxProductListingLifecycleGuardFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingLifecycleGuard + 'tx {
        SqlxProductListingLifecycleGuard {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingLifecycleGuard for SqlxProductListingLifecycleGuard<'_> {
    async fn lock_and_find_lifecycle(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ListingLifecycle>, ProductListingLifecycleGuardError> {
        let lifecycle = sqlx::query_scalar::<_, String>(
            "SELECT lifecycle FROM product_listings WHERE product_listing_id = $1 FOR SHARE",
        )
        .bind(product_listing_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(|source| ProductListingLifecycleGuardError::LockFailed {
            source: box_error(ProductListingLifecycleGuardSqlxError(source)),
        })?;

        lifecycle
            .map(|value| parse_listing_lifecycle(&value))
            .transpose()
    }
}

fn parse_listing_lifecycle(
    value: &str,
) -> Result<ListingLifecycle, ProductListingLifecycleGuardError> {
    ListingLifecycle::from_code(value)
        .ok_or(ProductListingLifecycleGuardError::InvalidListingLifecyclePersisted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn should_parse_all_canonical_listing_lifecycle_values() {
        for lifecycle in ListingLifecycle::iter() {
            assert!(matches!(
                parse_listing_lifecycle(lifecycle.as_str()),
                Ok(parsed) if parsed == lifecycle
            ));
        }
    }

    #[test]
    fn should_reject_noncanonical_persisted_listing_lifecycle() {
        assert!(matches!(
            parse_listing_lifecycle("active"),
            Err(ProductListingLifecycleGuardError::InvalidListingLifecyclePersisted)
        ));
    }

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error = ProductListingLifecycleGuardError::LockFailed {
            source: box_error(ProductListingLifecycleGuardSqlxError(
                sqlx::Error::RowNotFound,
            )),
        };

        let ProductListingLifecycleGuardError::LockFailed { source } = error else {
            panic!("expected lifecycle guard lock error");
        };
        assert!(
            source
                .downcast_ref::<ProductListingLifecycleGuardSqlxError>()
                .is_some()
        );
    }
}

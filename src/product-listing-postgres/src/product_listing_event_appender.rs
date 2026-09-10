use application::error::box_error;
use product_listing_service::ports::product_listing_event_appender::{
    ProductListingEvent, ProductListingEventAppendError, ProductListingEventAppender,
    ProductListingEventAppenderFactory,
};
use sqlx::PgConnection;

use crate::product_listing_event_codec::{self, PRODUCT_LISTING_EVENT_SCHEMA_VERSION};

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEventAppenderFactory;

struct SqlxProductListingEventAppender<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxProductListingEventAppenderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEventAppenderFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingEventAppenderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingEventAppender + 'tx {
        SqlxProductListingEventAppender {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingEventAppender for SqlxProductListingEventAppender<'_> {
    async fn append(
        &mut self,
        event: &ProductListingEvent,
    ) -> Result<(), ProductListingEventAppendError> {
        sqlx::query(
            r#"
            INSERT INTO product_listing_events (
                event_id,
                product_listing_id,
                event_type,
                event_group,
                event_type_schema_version,
                payload,
                event_time
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(event.event_id.as_uuid())
        .bind(event.aggregate_id.as_uuid())
        .bind(event.payload.event_type().as_str())
        .bind("DOMAIN")
        .bind(PRODUCT_LISTING_EVENT_SCHEMA_VERSION)
        .bind(
            product_listing_event_codec::encode(&event.payload).map_err(|error| {
                ProductListingEventAppendError::PayloadSerializationFailed {
                    source: box_error(error),
                }
            })?,
        )
        .bind(event.timestamp)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingEventAppendSqlxError)?;

        Ok(())
    }
}

struct ProductListingEventAppendSqlxError(sqlx::Error);

impl From<ProductListingEventAppendSqlxError> for ProductListingEventAppendError {
    fn from(value: ProductListingEventAppendSqlxError) -> Self {
        let ProductListingEventAppendSqlxError(error) = value;
        match &error {
            sqlx::Error::Database(db_error) if db_error.is_unique_violation() => {
                Self::ProductListingEventAlreadyExists
            }
            _ => Self::ProductListingEventAppendFailed {
                source: box_error(error),
            },
        }
    }
}

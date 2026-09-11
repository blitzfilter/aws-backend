use crate::auction_event_codec::{self, AUCTION_EVENT_SCHEMA_VERSION};
use application::error::box_error;
use auction_service::ports::{
    AuctionEvent, AuctionEventAppendError, AuctionEventAppender, AuctionEventAppenderFactory,
};
use platform_postgres::SqlxTransaction;
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxAuctionEventAppenderFactory;

struct SqlxAuctionEventAppender<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxAuctionEventAppenderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl AuctionEventAppenderFactory<SqlxTransaction> for SqlxAuctionEventAppenderFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl AuctionEventAppender + 'tx {
        SqlxAuctionEventAppender {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl AuctionEventAppender for SqlxAuctionEventAppender<'_> {
    async fn append(&mut self, event: &AuctionEvent) -> Result<(), AuctionEventAppendError> {
        let payload = auction_event_codec::encode(&event.payload).map_err(|error| {
            AuctionEventAppendError::PayloadSerializationFailed {
                source: auction_event_codec::boxed(error),
            }
        })?;
        sqlx::query("INSERT INTO auction_events (event_id, auction_id, event_type, event_type_schema_version, payload, event_time) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(event.event_id.as_uuid())
            .bind(event.aggregate_id.as_uuid())
            .bind(event.payload.event_type().as_str())
            .bind(AUCTION_EVENT_SCHEMA_VERSION)
            .bind(payload)
            .bind(event.timestamp)
            .execute(&mut *self.connection).await.map_err(|error| match &error {
                sqlx::Error::Database(database) if database.is_unique_violation() => AuctionEventAppendError::AuctionEventAlreadyExists,
                _ => AuctionEventAppendError::AuctionEventAppendFailed { source: box_error(error) },
            })?;
        Ok(())
    }
}

use application::error::BoxError;
use auction_core::{AuctionEventPayload, AuctionId};
use domain_primitives::{event::Event, event_id::EventId};
use time::OffsetDateTime;

pub type AuctionEvent = Event<AuctionId, AuctionEventPayload>;

pub fn stamp_auction_event(
    auction_id: AuctionId,
    occurred_at: OffsetDateTime,
    payload: AuctionEventPayload,
) -> AuctionEvent {
    AuctionEvent {
        aggregate_id: auction_id,
        event_id: EventId::new(),
        timestamp: occurred_at,
        payload,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuctionEventAppendError {
    #[error("auction event already exists")]
    AuctionEventAlreadyExists,
    #[error("auction event payload serialization failed")]
    PayloadSerializationFailed {
        #[source]
        source: BoxError,
    },
    #[error("auction event append failed")]
    AuctionEventAppendFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuctionEventAppender: Send {
    async fn append(&mut self, event: &AuctionEvent) -> Result<(), AuctionEventAppendError>;
}

pub trait AuctionEventAppenderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl AuctionEventAppender + 'tx;
}

use crate::event_id::EventId;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct Event<AggregateId, Payload> {
    pub aggregate_id: AggregateId,
    pub event_id: EventId,
    pub timestamp: OffsetDateTime,
    pub payload: Payload,
}

use crate::event_id::EventId;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct Event<AggregateId, Payload> {
    pub aggregate_id: AggregateId,
    pub event_id: EventId,
    pub timestamp: OffsetDateTime,
    pub payload: Payload,
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, Rng};

    impl<AggregateId: Dummy<Faker>, Payload: Dummy<Faker>> Dummy<Faker>
        for Event<AggregateId, Payload>
    {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            Event {
                aggregate_id: config.fake_with_rng(rng),
                event_id: config.fake_with_rng(rng),
                timestamp: OffsetDateTime::now_utc(),
                payload: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::event::Event;
        use fake::{Fake, Faker};
        use uuid::Uuid;

        #[test]
        fn should_fake_event() {
            let _ = Faker.fake::<Event<Uuid, String>>();
        }
    }
}

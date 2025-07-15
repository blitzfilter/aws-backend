pub trait AggregateError {
    fn empty() -> Self;
}

pub trait Aggregate<Event> {
    type Error: AggregateError;

    fn init(event: Event) -> Result<Self, Self::Error>
    where
        Self: Sized;

    fn apply(&mut self, event: Event) -> Result<(), Self::Error>;

    fn rehydrate(&mut self, events: impl IntoIterator<Item = Event>) -> Result<(), Self::Error> {
        for event in events {
            self.apply(event)?;
        }
        Ok(())
    }

    fn replay(events: impl IntoIterator<Item = Event>) -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        let mut iter = events.into_iter();
        let first = iter.next().ok_or(Self::Error::empty())?;
        let mut agg = Self::init(first)?;

        agg.rehydrate(iter)?;
        Ok(agg)
    }
}

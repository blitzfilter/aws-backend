use crate::AuctionTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionSchedulePoint {
    BiddingOpens,
    LiveStarts,
    LotsBeginClosing,
    ScheduledEnd,
}

impl AuctionSchedulePoint {
    const fn label(self) -> &'static str {
        match self {
            Self::BiddingOpens => "bidding opens",
            Self::LiveStarts => "live starts",
            Self::LotsBeginClosing => "lots begin closing",
            Self::ScheduledEnd => "scheduled end",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("auction schedule {first} is after {second}", first = .first.label(), second = .second.label())]
pub struct InvalidAuctionSchedule {
    first: AuctionSchedulePoint,
    second: AuctionSchedulePoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuctionSchedule {
    bidding_opens: Option<AuctionTime>,
    live_starts: Option<AuctionTime>,
    lots_begin_closing: Option<AuctionTime>,
    scheduled_end: Option<AuctionTime>,
}

impl AuctionSchedule {
    pub fn new(
        bidding_opens: Option<AuctionTime>,
        live_starts: Option<AuctionTime>,
        lots_begin_closing: Option<AuctionTime>,
        scheduled_end: Option<AuctionTime>,
    ) -> Result<Self, InvalidAuctionSchedule> {
        let schedule = Self {
            bidding_opens,
            live_starts,
            lots_begin_closing,
            scheduled_end,
        };
        schedule.validate()?;
        Ok(schedule)
    }

    pub fn bidding_opens(&self) -> Option<&AuctionTime> {
        self.bidding_opens.as_ref()
    }

    pub fn live_starts(&self) -> Option<&AuctionTime> {
        self.live_starts.as_ref()
    }

    pub fn lots_begin_closing(&self) -> Option<&AuctionTime> {
        self.lots_begin_closing.as_ref()
    }

    pub fn scheduled_end(&self) -> Option<&AuctionTime> {
        self.scheduled_end.as_ref()
    }

    fn validate(&self) -> Result<(), InvalidAuctionSchedule> {
        for (point, value) in [
            (
                AuctionSchedulePoint::BiddingOpens,
                self.bidding_opens.as_ref(),
            ),
            (AuctionSchedulePoint::LiveStarts, self.live_starts.as_ref()),
            (
                AuctionSchedulePoint::LotsBeginClosing,
                self.lots_begin_closing.as_ref(),
            ),
        ] {
            if let (Some(value), Some(end)) = (value, self.scheduled_end.as_ref())
                && value.is_after_in_same_precision_context(end)
            {
                return Err(InvalidAuctionSchedule {
                    first: point,
                    second: AuctionSchedulePoint::ScheduledEnd,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AuctionSchedule, AuctionSchedulePoint, InvalidAuctionSchedule};
    use crate::{AuctionTime, AuctionTimeZone};
    use time::{Date, Month, macros::datetime};

    fn berlin_timezone() -> AuctionTimeZone {
        AuctionTimeZone::try_from("Europe/Berlin")
            .unwrap_or_else(|error| panic!("valid test timezone: {error}"))
    }

    #[test]
    fn should_accept_an_empty_schedule() {
        assert_eq!(
            Ok(AuctionSchedule::default()),
            AuctionSchedule::new(None, None, None, None)
        );
    }

    #[test]
    fn should_reject_comparable_exact_milestone_after_scheduled_end() {
        let schedule = AuctionSchedule::new(
            Some(AuctionTime::instant(
                datetime!(2026-05-13 11:00 +02:00),
                Some(berlin_timezone()),
            )),
            None,
            None,
            Some(AuctionTime::instant(
                datetime!(2026-05-13 10:00 +02:00),
                Some(berlin_timezone()),
            )),
        );

        assert_eq!(
            Err(InvalidAuctionSchedule {
                first: AuctionSchedulePoint::BiddingOpens,
                second: AuctionSchedulePoint::ScheduledEnd,
            }),
            schedule
        );
    }

    #[test]
    fn should_compare_source_dates_only_in_the_same_declared_calendar_context() {
        let earlier = Date::from_calendar_date(2026, Month::May, 13)
            .unwrap_or_else(|error| panic!("valid test date: {error}"));
        let later = Date::from_calendar_date(2026, Month::May, 14)
            .unwrap_or_else(|error| panic!("valid test date: {error}"));

        assert!(
            AuctionSchedule::new(
                Some(AuctionTime::date(later, Some(berlin_timezone()))),
                None,
                None,
                Some(AuctionTime::date(earlier, Some(berlin_timezone()))),
            )
            .is_err()
        );
        assert!(
            AuctionSchedule::new(
                Some(AuctionTime::date(later, None)),
                None,
                None,
                Some(AuctionTime::date(earlier, None)),
            )
            .is_ok()
        );
    }

    #[test]
    fn should_not_invent_a_midnight_comparison_for_mixed_precision() {
        let date = Date::from_calendar_date(2026, Month::May, 13)
            .unwrap_or_else(|error| panic!("valid test date: {error}"));

        assert!(
            AuctionSchedule::new(
                Some(AuctionTime::date(date, Some(berlin_timezone()))),
                None,
                None,
                Some(AuctionTime::instant(
                    datetime!(2026-05-12 23:00 UTC),
                    Some(berlin_timezone()),
                )),
            )
            .is_ok()
        );
    }
}

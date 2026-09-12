use auction_core::{AuctionId, AuctionTime};
use std::fmt;
use time::OffsetDateTime;

const MAX_LOT_NUMBER_BYTES: usize = 128;

/// Source-assigned lot label. It is not an Auction aggregate membership reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LotNumber(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidLotNumber {
    #[error("lot number cannot be blank")]
    Blank,
    #[error("lot number cannot contain a NUL character")]
    ContainsNul,
    #[error("lot number exceeds {MAX_LOT_NUMBER_BYTES} UTF-8 bytes")]
    TooLong,
}

impl LotNumber {
    pub fn parse(value: &str) -> Result<Self, InvalidLotNumber> {
        let value = value.trim();
        if value.is_empty() {
            return Err(InvalidLotNumber::Blank);
        }
        if value.contains('\0') {
            return Err(InvalidLotNumber::ContainsNul);
        }
        if value.len() > MAX_LOT_NUMBER_BYTES {
            return Err(InvalidLotNumber::TooLong);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for LotNumber {
    type Error = InvalidLotNumber;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for LotNumber {
    type Error = InvalidLotNumber;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl fmt::Display for LotNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<LotNumber> for String {
    fn from(value: LotNumber) -> Self {
        value.0
    }
}

/// One-based source catalogue ordering for a lot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CataloguePosition(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidCataloguePosition {
    #[error("catalogue position must be greater than zero")]
    Zero,
    #[error("catalogue position exceeds u32")]
    TooLarge,
}

impl CataloguePosition {
    pub const fn new(value: u32) -> Result<Self, InvalidCataloguePosition> {
        if value == 0 {
            return Err(InvalidCataloguePosition::Zero);
        }
        Ok(Self(value))
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

impl TryFrom<u64> for CataloguePosition {
    type Error = InvalidCataloguePosition;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        let value = u32::try_from(value).map_err(|_| InvalidCataloguePosition::TooLarge)?;
        Self::new(value)
    }
}

/// Optional timing assertions for one lot.
///
/// Exact instants are compared directly. Source dates are compared only when both
/// carry the same declared calendar timezone. Mixed precision and timezone-less
/// dates remain intentionally incomparable. A reported closure is always exact;
/// it is an observed fact rather than a scheduled boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LotAuctionTiming {
    bidding_opens: Option<AuctionTime>,
    scheduled_closes: Option<AuctionTime>,
    reported_closed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidLotAuctionTiming {
    #[error("lot auction bidding opens after its scheduled close")]
    BiddingOpensAfterScheduledCloses,
}

impl LotAuctionTiming {
    pub fn new(
        bidding_opens: Option<AuctionTime>,
        scheduled_closes: Option<AuctionTime>,
        reported_closed_at: Option<OffsetDateTime>,
    ) -> Result<Self, InvalidLotAuctionTiming> {
        if bidding_opens
            .as_ref()
            .zip(scheduled_closes.as_ref())
            .is_some_and(|(bidding_opens, scheduled_closes)| {
                is_after_in_comparable_context(bidding_opens, scheduled_closes)
            })
        {
            return Err(InvalidLotAuctionTiming::BiddingOpensAfterScheduledCloses);
        }
        Ok(Self {
            bidding_opens,
            scheduled_closes,
            reported_closed_at,
        })
    }

    pub fn bidding_opens(&self) -> Option<&AuctionTime> {
        self.bidding_opens.as_ref()
    }

    pub fn scheduled_closes(&self) -> Option<&AuctionTime> {
        self.scheduled_closes.as_ref()
    }

    pub const fn reported_closed_at(&self) -> Option<OffsetDateTime> {
        self.reported_closed_at
    }
}

/// Resolved, same-source Auction membership for one listing context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuctionMembership {
    auction_id: AuctionId,
}

impl AuctionMembership {
    pub const fn new(auction_id: AuctionId) -> Self {
        Self { auction_id }
    }

    pub const fn auction_id(self) -> AuctionId {
        self.auction_id
    }
}

/// Optional source assertions about the auction context of this listing.
///
/// `None` outer context means no participation assertion. A present context with
/// no membership is an unresolved auction offering; an empty present context is
/// still a participation assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingAuction {
    membership: Option<AuctionMembership>,
    lot_number: Option<LotNumber>,
    catalogue_position: Option<CataloguePosition>,
    timing: Option<LotAuctionTiming>,
}

impl ProductListingAuction {
    pub const fn new(
        membership: Option<AuctionMembership>,
        lot_number: Option<LotNumber>,
        catalogue_position: Option<CataloguePosition>,
        timing: Option<LotAuctionTiming>,
    ) -> Self {
        Self {
            membership,
            lot_number,
            catalogue_position,
            timing,
        }
    }

    pub const fn membership(&self) -> Option<AuctionMembership> {
        self.membership
    }

    pub fn lot_number(&self) -> Option<&LotNumber> {
        self.lot_number.as_ref()
    }

    pub const fn catalogue_position(&self) -> Option<CataloguePosition> {
        self.catalogue_position
    }

    pub fn timing(&self) -> Option<&LotAuctionTiming> {
        self.timing.as_ref()
    }
}

fn is_after_in_comparable_context(left: &AuctionTime, right: &AuctionTime) -> bool {
    match (
        left.exact_instant(),
        right.exact_instant(),
        left.source_date(),
        right.source_date(),
        left.source_timezone(),
        right.source_timezone(),
    ) {
        (Some(left), Some(right), _, _, _, _) => left > right,
        (_, _, Some(left), Some(right), Some(left_timezone), Some(right_timezone))
            if left_timezone == right_timezone =>
        {
            left > right
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::AuctionTimeZone;
    use time::{Date, Month, macros::datetime};

    #[test]
    fn should_validate_lot_number_and_catalogue_position() {
        assert_eq!(
            Err(InvalidLotNumber::Blank),
            LotNumber::try_from(" \u{2003} ")
        );
        assert_eq!(
            Err(InvalidLotNumber::ContainsNul),
            LotNumber::try_from("12\0A")
        );
        assert_eq!(
            Err(InvalidCataloguePosition::Zero),
            CataloguePosition::new(0)
        );
        assert_eq!(
            Err(InvalidCataloguePosition::TooLarge),
            CataloguePosition::try_from(u64::from(u32::MAX) + 1)
        );
    }

    #[test]
    fn should_preserve_date_precision_and_reject_comparable_open_after_close() {
        let timezone = AuctionTimeZone::try_from("Europe/Berlin")
            .unwrap_or_else(|error| panic!("valid timezone: {error}"));
        let open = AuctionTime::date(
            Date::from_calendar_date(2026, Month::May, 14)
                .unwrap_or_else(|error| panic!("valid date: {error}")),
            Some(timezone.clone()),
        );
        let close = AuctionTime::date(
            Date::from_calendar_date(2026, Month::May, 13)
                .unwrap_or_else(|error| panic!("valid date: {error}")),
            Some(timezone),
        );

        assert_eq!(
            Some(
                Date::from_calendar_date(2026, Month::May, 14)
                    .unwrap_or_else(|error| panic!("valid date: {error}"))
            ),
            open.source_date()
        );
        assert_eq!(None, open.exact_instant());
        assert_eq!(
            Err(InvalidLotAuctionTiming::BiddingOpensAfterScheduledCloses),
            LotAuctionTiming::new(Some(open), Some(close), None)
        );
    }

    #[test]
    fn should_not_compare_mixed_or_timezone_less_date_precision() {
        let date = Date::from_calendar_date(2026, Month::May, 14)
            .unwrap_or_else(|error| panic!("valid date: {error}"));
        let earlier = Date::from_calendar_date(2026, Month::May, 13)
            .unwrap_or_else(|error| panic!("valid date: {error}"));

        assert!(
            LotAuctionTiming::new(
                Some(AuctionTime::date(date, None)),
                Some(AuctionTime::date(earlier, None)),
                None,
            )
            .is_ok()
        );
        assert!(
            LotAuctionTiming::new(
                Some(AuctionTime::date(date, None)),
                Some(AuctionTime::instant(datetime!(2026-05-13 23:00 UTC), None)),
                None,
            )
            .is_ok()
        );
    }
}

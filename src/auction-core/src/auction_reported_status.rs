use std::str::FromStr;
use strum::IntoEnumIterator;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum AuctionReportedStatus {
    Scheduled,
    InProgress,
    Ended,
    Postponed,
    Cancelled,
}

impl AuctionReportedStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "SCHEDULED",
            Self::InProgress => "IN_PROGRESS",
            Self::Ended => "ENDED",
            Self::Postponed => "POSTPONED",
            Self::Cancelled => "CANCELLED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid auction reported status `{value}`")]
pub struct InvalidAuctionReportedStatus {
    value: String,
}

impl FromStr for AuctionReportedStatus {
    type Err = InvalidAuctionReportedStatus;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::iter()
            .find(|status| status.as_str() == value)
            .ok_or_else(|| InvalidAuctionReportedStatus {
                value: value.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::AuctionReportedStatus;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_exact_unique_auction_reported_status_codes() {
        let statuses = AuctionReportedStatus::iter().collect::<Vec<_>>();

        assert_eq!(
            statuses.len(),
            statuses
                .iter()
                .map(|status| status.as_str())
                .collect::<HashSet<_>>()
                .len()
        );
        for status in statuses {
            assert_eq!(Ok(status), status.as_str().parse());
        }
        assert!("scheduled".parse::<AuctionReportedStatus>().is_err());
        assert!("UNKNOWN".parse::<AuctionReportedStatus>().is_err());
    }
}

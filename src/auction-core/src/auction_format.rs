use std::str::FromStr;
use strum::IntoEnumIterator;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum_macros::EnumIter)]
pub enum AuctionFormat {
    Live,
    Timed,
}

impl AuctionFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "LIVE",
            Self::Timed => "TIMED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid auction format `{value}`")]
pub struct InvalidAuctionFormat {
    value: String,
}

impl FromStr for AuctionFormat {
    type Err = InvalidAuctionFormat;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::iter()
            .find(|format| format.as_str() == value)
            .ok_or_else(|| InvalidAuctionFormat {
                value: value.to_owned(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::AuctionFormat;
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_exact_unique_auction_format_codes() {
        let formats = AuctionFormat::iter().collect::<Vec<_>>();

        assert_eq!(
            formats.len(),
            formats
                .iter()
                .map(|format| format.as_str())
                .collect::<HashSet<_>>()
                .len()
        );
        assert_eq!(Ok(AuctionFormat::Live), "LIVE".parse());
        assert_eq!(Ok(AuctionFormat::Timed), "TIMED".parse());
        assert!("live".parse::<AuctionFormat>().is_err());
        assert!("ONLINE".parse::<AuctionFormat>().is_err());
    }
}

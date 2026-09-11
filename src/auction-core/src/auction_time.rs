use std::fmt;
use time::{Date, OffsetDateTime};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuctionTimeZone(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAuctionTimeZone {
    #[error("auction timezone cannot be blank")]
    Blank,
    #[error("invalid IANA auction timezone `{value}`")]
    Invalid { value: String },
}

impl AuctionTimeZone {
    pub fn parse(value: &str) -> Result<Self, InvalidAuctionTimeZone> {
        if value.is_empty() {
            return Err(InvalidAuctionTimeZone::Blank);
        }
        use time_tz::TimeZone;

        let Some(timezone) = time_tz::timezones::get_by_name(value) else {
            return Err(InvalidAuctionTimeZone::Invalid {
                value: value.to_owned(),
            });
        };
        if timezone.name() != value {
            return Err(InvalidAuctionTimeZone::Invalid {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for AuctionTimeZone {
    type Error = InvalidAuctionTimeZone;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for AuctionTimeZone {
    type Error = InvalidAuctionTimeZone;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl fmt::Display for AuctionTimeZone {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<AuctionTimeZone> for String {
    fn from(value: AuctionTimeZone) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuctionTime {
    Instant {
        at: OffsetDateTime,
        source_timezone: Option<AuctionTimeZone>,
    },
    Date {
        on: Date,
        source_timezone: Option<AuctionTimeZone>,
    },
}

impl AuctionTime {
    pub const fn instant(at: OffsetDateTime, source_timezone: Option<AuctionTimeZone>) -> Self {
        Self::Instant {
            at,
            source_timezone,
        }
    }

    pub const fn date(on: Date, source_timezone: Option<AuctionTimeZone>) -> Self {
        Self::Date {
            on,
            source_timezone,
        }
    }

    pub const fn source_timezone(&self) -> Option<&AuctionTimeZone> {
        match self {
            Self::Instant {
                source_timezone, ..
            }
            | Self::Date {
                source_timezone, ..
            } => source_timezone.as_ref(),
        }
    }

    pub const fn exact_instant(&self) -> Option<OffsetDateTime> {
        match self {
            Self::Instant { at, .. } => Some(*at),
            Self::Date { .. } => None,
        }
    }

    pub const fn source_date(&self) -> Option<Date> {
        match self {
            Self::Instant { .. } => None,
            Self::Date { on, .. } => Some(*on),
        }
    }

    pub(crate) fn is_after_in_same_precision_context(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Instant { at: left, .. }, Self::Instant { at: right, .. }) => left > right,
            (
                Self::Date {
                    on: left,
                    source_timezone: Some(left_timezone),
                },
                Self::Date {
                    on: right,
                    source_timezone: Some(right_timezone),
                },
            ) if left_timezone == right_timezone => left > right,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AuctionTime, AuctionTimeZone, InvalidAuctionTimeZone};
    use time::{Date, Month, macros::datetime};

    #[test]
    fn should_validate_iana_timezones_without_trimming_or_guessing() {
        let timezone = AuctionTimeZone::try_from("Europe/Berlin")
            .unwrap_or_else(|error| panic!("valid timezone: {error}"));

        assert_eq!("Europe/Berlin", timezone.as_str());
        assert!(AuctionTimeZone::try_from("Europe/NotAPlace").is_err());
        assert!(AuctionTimeZone::try_from("China Standard Time").is_err());
        assert_eq!(
            Err(InvalidAuctionTimeZone::Blank),
            AuctionTimeZone::try_from("")
        );
        assert!(AuctionTimeZone::try_from(" Europe/Berlin ").is_err());
    }

    #[test]
    fn should_keep_date_precision_without_creating_an_instant() {
        let date = Date::from_calendar_date(2026, Month::May, 13)
            .unwrap_or_else(|error| panic!("valid test date: {error}"));
        let value = AuctionTime::date(date, None);

        assert_eq!(Some(date), value.source_date());
        assert_eq!(None, value.exact_instant());
    }

    #[test]
    fn should_compare_exact_instants_by_instant() {
        let timezone = AuctionTimeZone::try_from("Europe/Berlin")
            .unwrap_or_else(|error| panic!("valid timezone: {error}"));
        let later = AuctionTime::instant(datetime!(2026-05-13 10:00 +02:00), Some(timezone));
        let earlier = AuctionTime::instant(datetime!(2026-05-13 07:00 UTC), None);

        assert!(later.is_after_in_same_precision_context(&earlier));
    }
}

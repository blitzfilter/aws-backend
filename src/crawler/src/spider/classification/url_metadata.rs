use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UrlClass {
    ProductListing,
    Category,
    Imprint,
    Info,
    Other,
}

impl std::fmt::Display for UrlClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlClass::ProductListing => write!(f, "product"),
            UrlClass::Category => write!(f, "category"),
            UrlClass::Imprint => write!(f, "imprint"),
            UrlClass::Info => write!(f, "info"),
            UrlClass::Other => write!(f, "other"),
        }
    }
}

impl std::str::FromStr for UrlClass {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "product" => Ok(UrlClass::ProductListing),
            "category" => Ok(UrlClass::Category),
            "imprint" => Ok(UrlClass::Imprint),
            "info" => Ok(UrlClass::Info),
            "other" => Ok(UrlClass::Other),
            _ => Err(format!("Invalid URL class: {s}")),
        }
    }
}

impl UrlClass {
    pub fn as_str(self) -> &'static str {
        match self {
            UrlClass::ProductListing => "product",
            UrlClass::Category => "category",
            UrlClass::Imprint => "imprint",
            UrlClass::Info => "info",
            UrlClass::Other => "other",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "product" => UrlClass::ProductListing,
            "category" => UrlClass::Category,
            "imprint" => UrlClass::Imprint,
            "info" => UrlClass::Info,
            _ => UrlClass::Other,
        }
    }
}

/// Durable crawler scheduling state. Only successful crawler handoffs may make a URL dormant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum_macros::EnumIter)]
pub enum CrawlerDisposition {
    Active,
    DormantSold,
    DormantRemoved,
}

impl CrawlerDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::DormantSold => "DORMANT_SOLD",
            Self::DormantRemoved => "DORMANT_REMOVED",
        }
    }
}

/// Result of a conditional crawler-local URL write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlerUrlWriteOutcome {
    Applied,
    NoopStale,
}

impl std::fmt::Display for CrawlerDisposition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for CrawlerDisposition {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        use strum::IntoEnumIterator;

        Self::iter()
            .find(|disposition| disposition.as_str() == value)
            .ok_or_else(|| format!("Invalid crawler disposition: {value}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledUrlMetadata {
    pub url: String,
    pub class: UrlClass,
    pub disposition: CrawlerDisposition,

    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub last_scraped: Option<OffsetDateTime>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::CrawlerDisposition;
    use strum::IntoEnumIterator;

    #[test]
    fn should_round_trip_exact_disposition_codes() {
        for disposition in CrawlerDisposition::iter() {
            assert_eq!(disposition.as_str().parse(), Ok(disposition));
        }
        assert!("active".parse::<CrawlerDisposition>().is_err());
    }
}

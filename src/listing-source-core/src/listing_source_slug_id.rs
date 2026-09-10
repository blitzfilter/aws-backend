use std::fmt::{Display, Formatter};

use crate::ListingSourceId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid listing source slug '{value}'")]
pub struct InvalidListingSourceSlug {
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListingSourceSlugId(String);

impl ListingSourceSlugId {
    pub const FALLBACK_PREFIX: &str = "listing-source";

    pub fn raw(value: impl AsRef<str>) -> Result<Self, InvalidListingSourceSlug> {
        let value = value.as_ref();
        if !value.is_empty()
            && !value.starts_with('-')
            && !value.ends_with('-')
            && !value.contains("--")
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidListingSourceSlug {
                value: value.to_owned(),
            })
        }
    }

    pub(crate) fn derive(name: &str, listing_source_id: ListingSourceId) -> Self {
        let prefix = slug::slugify(name);
        let prefix = if prefix.is_empty() {
            Self::FALLBACK_PREFIX
        } else {
            prefix.as_str()
        };

        Self(format!("{prefix}-{}", listing_source_id.as_uuid()))
    }
}

impl AsRef<str> for ListingSourceSlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for ListingSourceSlugId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

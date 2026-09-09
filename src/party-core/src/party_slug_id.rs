use crate::party_id::PartyId;
use std::{fmt::Display, ops::Deref};

#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid party slug '{value}'")]
pub struct InvalidPartySlugId {
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartySlugId(String);

impl PartySlugId {
    pub const FALLBACK_PREFIX: &str = "party";

    pub fn raw(value: impl AsRef<str>) -> Result<Self, InvalidPartySlugId> {
        let value = value.as_ref();
        if !value.is_empty()
            && !value.starts_with('-')
            && !value.ends_with('-')
            && !value.contains("--")
            && value.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
        {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidPartySlugId {
                value: value.to_owned(),
            })
        }
    }

    pub(crate) fn derive(name: &str, party_id: PartyId) -> Self {
        let prefix = slug::slugify(name);
        let prefix = if prefix.is_empty() {
            Self::FALLBACK_PREFIX
        } else {
            prefix.as_str()
        };

        Self(format!("{prefix}-{}", party_id.as_uuid()))
    }
}

impl Display for PartySlugId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for PartySlugId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Deref for PartySlugId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

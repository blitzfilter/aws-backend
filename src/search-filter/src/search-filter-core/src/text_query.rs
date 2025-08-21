use std::fmt::Display;
use std::ops::Deref;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("String is too short for TextQuery, got '{0}', expected at least '3'.")]
pub struct TextQueryTooShortError(usize);

#[derive(Debug, Clone, PartialEq)]
pub struct TextQuery(String);

impl TryFrom<&str> for TextQuery {
    type Error = TextQueryTooShortError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s.len() {
            l @ ..3 => Err(TextQueryTooShortError(l)),
            3..256 => Ok(Self(s.into())),
            _ => match s.split_at_checked(255) {
                Some((truncated, _)) => Ok(Self(truncated.into())),
                None => Ok(Self(s.into())),
            },
        }
    }
}

impl Display for TextQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<String> for TextQuery {
    type Error = TextQueryTooShortError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

impl From<TextQuery> for String {
    fn from(t: TextQuery) -> Self {
        t.0
    }
}

impl Deref for TextQuery {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for TextQuery {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

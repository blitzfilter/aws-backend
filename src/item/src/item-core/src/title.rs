use std::fmt::{Debug, Display};
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub struct Title(String);

impl From<&str> for Title {
    fn from(s: &str) -> Self {
        if s.len() > 255 {
            match s.split_at_checked(255) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(s.into()),
            }
        } else {
            Title(s.into())
        }
    }
}

impl Display for Title {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Title {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<Title> for String {
    fn from(t: Title) -> Self {
        t.0
    }
}

impl Deref for Title {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Title {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

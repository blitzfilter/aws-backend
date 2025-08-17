use std::fmt::Display;
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub struct TextQuery(String);

impl From<&str> for TextQuery {
    fn from(s: &str) -> Self {
        if s.len() > 255 {
            match s.split_at_checked(255) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(s.into()),
            }
        } else {
            TextQuery(s.into())
        }
    }
}

impl Display for TextQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TextQuery {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
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

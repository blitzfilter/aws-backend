use std::fmt::Display;
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub struct Description(String);

impl From<&str> for Description {
    fn from(s: &str) -> Self {
        if s.len() > 4000 {
            match s.split_at_checked(4000) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(s.into()),
            }
        } else {
            Description(s.into())
        }
    }
}

impl Display for Description {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Description {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<Description> for String {
    fn from(t: Description) -> Self {
        t.0
    }
}

impl Deref for Description {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Description {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

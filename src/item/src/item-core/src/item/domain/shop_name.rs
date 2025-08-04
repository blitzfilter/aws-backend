use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub struct ShopName(String);

impl From<&str> for ShopName {
    fn from(s: &str) -> Self {
        if s.len() > 255 {
            match s.split_at_checked(255) {
                Some((truncated, _)) => Self(truncated.into()),
                None => Self(s.into()),
            }
        } else {
            ShopName(s.into())
        }
    }
}

impl From<String> for ShopName {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

impl From<ShopName> for String {
    fn from(t: ShopName) -> Self {
        t.0
    }
}

impl Deref for ShopName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ShopName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

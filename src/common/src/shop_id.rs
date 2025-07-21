use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShopId(String);

impl Default for ShopId {
    fn default() -> Self {
        Self::new()
    }
}

impl ShopId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Display for ShopId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ShopId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ShopId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<ShopId> for String {
    fn from(id: ShopId) -> Self {
        id.0
    }
}

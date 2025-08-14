use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShopsItemId(String);

impl ShopsItemId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for ShopsItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for ShopsItemId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<ShopsItemId> for String {
    fn from(id: ShopsItemId) -> Self {
        id.0
    }
}

impl From<String> for ShopsItemId {
    fn from(value: String) -> Self {
        ShopsItemId(value)
    }
}

impl From<&String> for ShopsItemId {
    fn from(value: &String) -> Self {
        ShopsItemId(value.to_owned())
    }
}

impl From<&str> for ShopsItemId {
    fn from(value: &str) -> Self {
        ShopsItemId(value.to_owned())
    }
}

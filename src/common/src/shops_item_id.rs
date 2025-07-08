use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct ShopsItemId(Uuid);

impl Default for ShopsItemId {
    fn default() -> Self {
        Self::new()
    }
}

impl ShopsItemId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Display for ShopsItemId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<String> for ShopsItemId {
    type Error = uuid::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&s).map(Self)
    }
}

impl From<ShopsItemId> for String {
    fn from(id: ShopsItemId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<&str> for ShopsItemId {
    type Error = uuid::Error;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Serialize, Deserialize)]
pub struct ShopsItemId(String);

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

impl From<&str> for ShopsItemId {
    fn from(value: &str) -> Self {
        ShopsItemId(value.to_string())
    }
}

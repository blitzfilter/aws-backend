use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct EventId(Uuid);

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

impl EventId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Display for EventId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<String> for EventId {
    type Error = uuid::Error;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Uuid::parse_str(&s).map(Self)
    }
}

impl From<EventId> for String {
    fn from(id: EventId) -> Self {
        id.0.to_string()
    }
}

impl TryFrom<&str> for EventId {
    type Error = uuid::Error;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Uuid::parse_str(s).map(Self)
    }
}

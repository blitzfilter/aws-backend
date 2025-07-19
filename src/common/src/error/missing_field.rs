#[derive(Debug, Clone, Copy)]
pub struct MissingPersistenceField(&'static str);

impl MissingPersistenceField {
    pub fn new(field: &'static str) -> Self {
        MissingPersistenceField(field)
    }
}

impl std::fmt::Display for MissingPersistenceField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for MissingPersistenceField {}

impl From<&'static str> for MissingPersistenceField {
    fn from(value: &'static str) -> Self {
        MissingPersistenceField(value)
    }
}

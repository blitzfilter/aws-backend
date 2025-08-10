use time::error::Format;

use crate::error::missing_field::MissingPersistenceField;

#[derive(thiserror::Error, Debug)]
pub enum PersistenceMappingError {
    #[error("MissingPersistenceField: {0}")]
    MissingPersistenceField(#[from] MissingPersistenceField),

    #[error("TimeFormatError: {0}")]
    TimeFormatError(#[from] Format),
}

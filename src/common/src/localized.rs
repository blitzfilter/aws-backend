use crate::language::{domain::Language, record::TextRecord};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, PartialEq)]
pub struct Localized<L, T> {
    pub localization: L,
    pub payload: T,
}

impl<L, T> Localized<L, T> {
    pub fn new(localization: L, payload: T) -> Localized<L, T> {
        Localized {
            localization,
            payload,
        }
    }
}

impl<T: From<String>> From<TextRecord> for Localized<Language, T> {
    fn from(value: TextRecord) -> Self {
        Localized {
            localization: value.language.into(),
            payload: value.text.into(),
        }
    }
}

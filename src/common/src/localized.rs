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

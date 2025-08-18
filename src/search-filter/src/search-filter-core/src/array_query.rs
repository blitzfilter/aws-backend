use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct AnyOfQuery<T>(pub HashSet<T>);

impl<T> Default for AnyOfQuery<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

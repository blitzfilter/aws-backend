#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default)]
pub struct RangeQuery<T: Ord> {
    pub min: Option<T>,
    pub max: Option<T>,
}

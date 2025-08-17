#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct RangeQuery<T: Ord> {
    pub from: Option<T>,
    pub to: Option<T>,
}

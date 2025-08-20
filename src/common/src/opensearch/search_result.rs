#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult<T> {
    pub hits: Vec<T>,
    pub total: u64,
}

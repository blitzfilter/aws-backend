use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollectionData<T> {
    pub items: Vec<T>,
    pub pagination: PaginationData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PaginationData {
    pub from: u64,
    pub size: u64,
    pub total: u64,
}

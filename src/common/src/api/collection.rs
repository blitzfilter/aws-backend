use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollectionData<T> {
    pub items: Vec<T>,
    pub pagination: PaginationData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PaginationData {
    pub from: u16,
    pub size: u16,
    pub total: u32,
}

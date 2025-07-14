#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ItemState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

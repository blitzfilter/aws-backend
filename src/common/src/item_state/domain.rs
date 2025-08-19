#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum ItemState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
}

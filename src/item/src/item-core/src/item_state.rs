#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ItemState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
}

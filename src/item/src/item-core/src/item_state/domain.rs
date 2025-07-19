use crate::item_state::command_data::ItemStateCommandData;
use crate::item_state::record::ItemStateRecord;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ItemState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ItemStateCommandData> for ItemState {
    fn from(cmd: ItemStateCommandData) -> Self {
        match cmd {
            ItemStateCommandData::Listed => ItemState::Listed,
            ItemStateCommandData::Available => ItemState::Available,
            ItemStateCommandData::Reserved => ItemState::Reserved,
            ItemStateCommandData::Sold => ItemState::Sold,
            ItemStateCommandData::Removed => ItemState::Removed,
            ItemStateCommandData::Unknown => ItemState::Unknown,
        }
    }
}

impl From<ItemStateRecord> for ItemState {
    fn from(cmd: ItemStateRecord) -> Self {
        match cmd {
            ItemStateRecord::Listed => ItemState::Listed,
            ItemStateRecord::Available => ItemState::Available,
            ItemStateRecord::Reserved => ItemState::Reserved,
            ItemStateRecord::Sold => ItemState::Sold,
            ItemStateRecord::Removed => ItemState::Removed,
            ItemStateRecord::Unknown => ItemState::Unknown,
        }
    }
}

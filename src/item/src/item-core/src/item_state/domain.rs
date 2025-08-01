use crate::item_state::command_data::ItemStateCommandData;
use crate::item_state::data::ItemStateData;
use crate::item_state::record::ItemStateRecord;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ItemState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
}

impl From<ItemStateCommandData> for ItemState {
    fn from(cmd: ItemStateCommandData) -> Self {
        match cmd {
            ItemStateCommandData::Listed => ItemState::Listed,
            ItemStateCommandData::Available => ItemState::Available,
            ItemStateCommandData::Reserved => ItemState::Reserved,
            ItemStateCommandData::Sold => ItemState::Sold,
            ItemStateCommandData::Removed => ItemState::Removed,
        }
    }
}

impl From<ItemStateData> for ItemState {
    fn from(cmd: ItemStateData) -> Self {
        match cmd {
            ItemStateData::Listed => ItemState::Listed,
            ItemStateData::Available => ItemState::Available,
            ItemStateData::Reserved => ItemState::Reserved,
            ItemStateData::Sold => ItemState::Sold,
            ItemStateData::Removed => ItemState::Removed,
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
        }
    }
}

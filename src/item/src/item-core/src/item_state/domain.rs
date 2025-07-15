use crate::item_state::command::ItemStateCommand;

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ItemState {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

impl From<ItemStateCommand> for ItemState {
    fn from(cmd: ItemStateCommand) -> Self {
        match cmd {
            ItemStateCommand::Listed => ItemState::Listed,
            ItemStateCommand::Available => ItemState::Available,
            ItemStateCommand::Reserved => ItemState::Reserved,
            ItemStateCommand::Sold => ItemState::Sold,
            ItemStateCommand::Removed => ItemState::Removed,
            ItemStateCommand::Unknown => ItemState::Unknown,
        }
    }
}

use item_core::item_state_domain::ItemState;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemStateCommandData {
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

#[cfg(test)]
mod tests {
    use super::ItemStateCommandData;
    use rstest::rstest;

    #[rstest]
    #[case(ItemStateCommandData::Listed, "\"LISTED\"")]
    #[case(ItemStateCommandData::Available, "\"AVAILABLE\"")]
    #[case(ItemStateCommandData::Reserved, "\"RESERVED\"")]
    #[case(ItemStateCommandData::Sold, "\"SOLD\"")]
    #[case(ItemStateCommandData::Removed, "\"REMOVED\"")]
    fn should_serialize_item_state_command_in_screaming_snake_case(
        #[case] item_state_record: ItemStateCommandData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ItemStateCommandData::Listed)]
    #[case("\"AVAILABLE\"", ItemStateCommandData::Available)]
    #[case("\"RESERVED\"", ItemStateCommandData::Reserved)]
    #[case("\"SOLD\"", ItemStateCommandData::Sold)]
    #[case("\"REMOVED\"", ItemStateCommandData::Removed)]
    fn should_deserialize_item_state_command_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ItemStateCommandData,
    ) {
        let actual = serde_json::from_str::<ItemStateCommandData>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

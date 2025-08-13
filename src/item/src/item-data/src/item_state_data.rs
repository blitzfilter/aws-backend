use item_core::item_state::ItemState;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemStateData {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
}

impl From<ItemState> for ItemStateData {
    fn from(domain: ItemState) -> Self {
        match domain {
            ItemState::Listed => ItemStateData::Listed,
            ItemState::Available => ItemStateData::Available,
            ItemState::Reserved => ItemStateData::Reserved,
            ItemState::Sold => ItemStateData::Sold,
            ItemState::Removed => ItemStateData::Removed,
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

#[cfg(test)]
mod tests {
    use super::ItemStateData;
    use rstest::rstest;

    #[rstest]
    #[case(ItemStateData::Listed, "\"LISTED\"")]
    #[case(ItemStateData::Available, "\"AVAILABLE\"")]
    #[case(ItemStateData::Reserved, "\"RESERVED\"")]
    #[case(ItemStateData::Sold, "\"SOLD\"")]
    #[case(ItemStateData::Removed, "\"REMOVED\"")]
    fn should_serialize_item_state_data_in_screaming_snake_case(
        #[case] item_state_record: ItemStateData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ItemStateData::Listed)]
    #[case("\"AVAILABLE\"", ItemStateData::Available)]
    #[case("\"RESERVED\"", ItemStateData::Reserved)]
    #[case("\"SOLD\"", ItemStateData::Sold)]
    #[case("\"REMOVED\"", ItemStateData::Removed)]
    fn should_deserialize_item_state_data_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ItemStateData,
    ) {
        let actual = serde_json::from_str::<ItemStateData>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

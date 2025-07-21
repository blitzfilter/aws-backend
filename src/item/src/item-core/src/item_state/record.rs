use crate::item_state::domain::ItemState;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemStateRecord {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
}

impl From<ItemState> for ItemStateRecord {
    fn from(domain: ItemState) -> Self {
        match domain {
            ItemState::Listed => ItemStateRecord::Listed,
            ItemState::Available => ItemStateRecord::Available,
            ItemState::Reserved => ItemStateRecord::Reserved,
            ItemState::Sold => ItemStateRecord::Sold,
            ItemState::Removed => ItemStateRecord::Removed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ItemStateRecord;
    use rstest::rstest;

    #[rstest]
    #[case(ItemStateRecord::Listed, "\"LISTED\"")]
    #[case(ItemStateRecord::Available, "\"AVAILABLE\"")]
    #[case(ItemStateRecord::Reserved, "\"RESERVED\"")]
    #[case(ItemStateRecord::Sold, "\"SOLD\"")]
    #[case(ItemStateRecord::Removed, "\"REMOVED\"")]
    fn should_serialize_item_state_record_in_screaming_snake_case(
        #[case] item_state_record: ItemStateRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ItemStateRecord::Listed)]
    #[case("\"AVAILABLE\"", ItemStateRecord::Available)]
    #[case("\"RESERVED\"", ItemStateRecord::Reserved)]
    #[case("\"SOLD\"", ItemStateRecord::Sold)]
    #[case("\"REMOVED\"", ItemStateRecord::Removed)]
    fn should_deserialize_item_state_record_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ItemStateRecord,
    ) {
        let actual = serde_json::from_str::<ItemStateRecord>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

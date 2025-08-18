use common::item_state::domain::ItemState;
use item_dynamodb::item_state_record::ItemStateRecord;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemStateDocument {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
}

impl From<ItemStateRecord> for ItemStateDocument {
    fn from(document: ItemStateRecord) -> Self {
        match document {
            ItemStateRecord::Listed => ItemStateDocument::Listed,
            ItemStateRecord::Available => ItemStateDocument::Available,
            ItemStateRecord::Reserved => ItemStateDocument::Reserved,
            ItemStateRecord::Sold => ItemStateDocument::Sold,
            ItemStateRecord::Removed => ItemStateDocument::Removed,
        }
    }
}

impl From<ItemState> for ItemStateDocument {
    fn from(value: ItemState) -> Self {
        match value {
            ItemState::Listed => ItemStateDocument::Listed,
            ItemState::Available => ItemStateDocument::Available,
            ItemState::Reserved => ItemStateDocument::Reserved,
            ItemState::Sold => ItemStateDocument::Sold,
            ItemState::Removed => ItemStateDocument::Removed,
        }
    }
}

impl ItemStateDocument {
    pub fn as_str(&self) -> &'static str {
        match self {
            ItemStateDocument::Listed => "LISTED",
            ItemStateDocument::Available => "AVAILABLE",
            ItemStateDocument::Reserved => "RESERVED",
            ItemStateDocument::Sold => "SOLD",
            ItemStateDocument::Removed => "REMOVED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ItemStateDocument;
    use rstest::rstest;

    #[rstest]
    #[case(ItemStateDocument::Listed, "\"LISTED\"")]
    #[case(ItemStateDocument::Available, "\"AVAILABLE\"")]
    #[case(ItemStateDocument::Reserved, "\"RESERVED\"")]
    #[case(ItemStateDocument::Sold, "\"SOLD\"")]
    #[case(ItemStateDocument::Removed, "\"REMOVED\"")]
    fn should_serialize_item_state_document_in_screaming_snake_case(
        #[case] state: ItemStateDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&state).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ItemStateDocument::Listed)]
    #[case("\"AVAILABLE\"", ItemStateDocument::Available)]
    #[case("\"RESERVED\"", ItemStateDocument::Reserved)]
    #[case("\"SOLD\"", ItemStateDocument::Sold)]
    #[case("\"REMOVED\"", ItemStateDocument::Removed)]
    fn should_deserialize_item_state_document_in_screaming_snake_case(
        #[case] state: &str,
        #[case] expected: ItemStateDocument,
    ) {
        let actual = serde_json::from_str::<ItemStateDocument>(state).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case(ItemStateDocument::Listed)]
    #[case(ItemStateDocument::Available)]
    #[case(ItemStateDocument::Reserved)]
    #[case(ItemStateDocument::Sold)]
    #[case(ItemStateDocument::Removed)]
    fn should_as_str_match_serialiazed(#[case] state: ItemStateDocument) {
        let serialized = serde_json::to_string::<ItemStateDocument>(&state)
            .unwrap()
            .replace("\"", "");
        let as_str = state.as_str();
        assert_eq!(serialized, as_str);
    }
}

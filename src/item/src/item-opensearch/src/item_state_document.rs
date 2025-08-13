use item_dynamodb::item_state_record::ItemStateRecord;
use serde::{Deserialize, Serialize};

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
        #[case] item_state_record: ItemStateDocument,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ItemStateDocument::Listed)]
    #[case("\"AVAILABLE\"", ItemStateDocument::Available)]
    #[case("\"RESERVED\"", ItemStateDocument::Reserved)]
    #[case("\"SOLD\"", ItemStateDocument::Sold)]
    #[case("\"REMOVED\"", ItemStateDocument::Removed)]
    fn should_deserialize_item_state_document_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ItemStateDocument,
    ) {
        let actual = serde_json::from_str::<ItemStateDocument>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

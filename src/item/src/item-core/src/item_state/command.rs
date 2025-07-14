use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ItemStateCommand {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::ItemStateCommand;
    use rstest::rstest;

    #[rstest]
    #[case(ItemStateCommand::Listed, "\"LISTED\"")]
    #[case(ItemStateCommand::Available, "\"AVAILABLE\"")]
    #[case(ItemStateCommand::Reserved, "\"RESERVED\"")]
    #[case(ItemStateCommand::Sold, "\"SOLD\"")]
    #[case(ItemStateCommand::Removed, "\"REMOVED\"")]
    #[case(ItemStateCommand::Unknown, "\"UNKNOWN\"")]
    fn should_serialize_item_state_command_in_screaming_snake_case(
        #[case] item_state_record: ItemStateCommand,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&item_state_record).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"LISTED\"", ItemStateCommand::Listed)]
    #[case("\"AVAILABLE\"", ItemStateCommand::Available)]
    #[case("\"RESERVED\"", ItemStateCommand::Reserved)]
    #[case("\"SOLD\"", ItemStateCommand::Sold)]
    #[case("\"REMOVED\"", ItemStateCommand::Removed)]
    #[case("\"UNKNOWN\"", ItemStateCommand::Unknown)]
    fn should_deserialize_item_state_command_in_screaming_snake_case(
        #[case] currency: &str,
        #[case] expected: ItemStateCommand,
    ) {
        let actual = serde_json::from_str::<ItemStateCommand>(currency).unwrap();
        assert_eq!(actual, expected);
    }
}

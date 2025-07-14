use serde::{Deserialize, Serialize};

// ISO 639-1
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LanguageCommand {
    De,
    En,
    Fr,
    Es,
}

#[cfg(test)]
mod tests {
    use super::LanguageCommand;
    use rstest::rstest;

    #[rstest]
    #[case(LanguageCommand::De, "\"de\"")]
    #[case(LanguageCommand::En, "\"en\"")]
    #[case(LanguageCommand::Fr, "\"fr\"")]
    #[case(LanguageCommand::Es, "\"es\"")]
    fn should_serialize_language_according_to_iso_639_1(
        #[case] language: LanguageCommand,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&language).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"de\"", LanguageCommand::De)]
    #[case("\"en\"", LanguageCommand::En)]
    #[case("\"fr\"", LanguageCommand::Fr)]
    #[case("\"es\"", LanguageCommand::Es)]
    fn should_deserialize_language_according_to_iso_639_1(
        #[case] language: &str,
        #[case] expected: LanguageCommand,
    ) {
        let actual = serde_json::from_str::<LanguageCommand>(language).unwrap();
        assert_eq!(actual, expected);
    }
}

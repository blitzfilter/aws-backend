use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LanguageRecord {
    De,
    En,
    Fr,
    Es,
}

#[cfg(test)]
mod tests {
    use super::LanguageRecord;
    use rstest::rstest;

    #[rstest]
    #[case(LanguageRecord::De, "\"DE\"")]
    #[case(LanguageRecord::En, "\"EN\"")]
    #[case(LanguageRecord::Fr, "\"FR\"")]
    #[case(LanguageRecord::Es, "\"ES\"")]
    fn should_serialize_language_in_screaming_snake_case(
        #[case] language: LanguageRecord,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&language).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"DE\"", LanguageRecord::De)]
    #[case("\"EN\"", LanguageRecord::En)]
    #[case("\"FR\"", LanguageRecord::Fr)]
    #[case("\"ES\"", LanguageRecord::Es)]
    fn should_deserialize_language_in_screaming_snake_case(
        #[case] language: &str,
        #[case] expected: LanguageRecord,
    ) {
        let actual = serde_json::from_str::<LanguageRecord>(language).unwrap();
        assert_eq!(actual, expected);
    }
}

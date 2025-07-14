use serde::{Deserialize, Serialize};

// ISO 639-1
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LanguageData {
    De,
    En,
    Fr,
    Es,
}

#[cfg(test)]
mod tests {
    use super::LanguageData;
    use rstest::rstest;

    #[rstest]
    #[case(LanguageData::De, "\"de\"")]
    #[case(LanguageData::En, "\"en\"")]
    #[case(LanguageData::Fr, "\"fr\"")]
    #[case(LanguageData::Es, "\"es\"")]
    fn should_serialize_language_according_to_iso_639_1(
        #[case] language: LanguageData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&language).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"de\"", LanguageData::De)]
    #[case("\"en\"", LanguageData::En)]
    #[case("\"fr\"", LanguageData::Fr)]
    #[case("\"es\"", LanguageData::Es)]
    fn should_deserialize_language_according_to_iso_639_1(
        #[case] language: &str,
        #[case] expected: LanguageData,
    ) {
        let actual = serde_json::from_str::<LanguageData>(language).unwrap();
        assert_eq!(actual, expected);
    }
}

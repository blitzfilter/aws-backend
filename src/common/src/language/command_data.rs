use crate::language::data::LanguageData;
use serde::{Deserialize, Serialize};

// ISO 639-1
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LanguageCommandData {
    De,
    En,
    Fr,
    Es,
}

impl From<LanguageData> for LanguageCommandData {
    fn from(data: LanguageData) -> Self {
        match data {
            LanguageData::De => LanguageCommandData::De,
            LanguageData::En => LanguageCommandData::En,
            LanguageData::Fr => LanguageCommandData::Fr,
            LanguageData::Es => LanguageCommandData::Es,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LanguageCommandData;
    use rstest::rstest;

    #[rstest]
    #[case(LanguageCommandData::De, "\"de\"")]
    #[case(LanguageCommandData::En, "\"en\"")]
    #[case(LanguageCommandData::Fr, "\"fr\"")]
    #[case(LanguageCommandData::Es, "\"es\"")]
    fn should_serialize_language_according_to_iso_639_1(
        #[case] language: LanguageCommandData,
        #[case] expected: &str,
    ) {
        let actual = serde_json::to_string(&language).unwrap();
        assert_eq!(actual, expected);
    }

    #[rstest]
    #[case("\"de\"", LanguageCommandData::De)]
    #[case("\"en\"", LanguageCommandData::En)]
    #[case("\"fr\"", LanguageCommandData::Fr)]
    #[case("\"es\"", LanguageCommandData::Es)]
    fn should_deserialize_language_according_to_iso_639_1(
        #[case] language: &str,
        #[case] expected: LanguageCommandData,
    ) {
        let actual = serde_json::from_str::<LanguageCommandData>(language).unwrap();
        assert_eq!(actual, expected);
    }
}

use crate::language::domain::Language;
use crate::localized::Localized;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LanguageRecord {
    De,
    En,
    Fr,
    Es,
}

impl From<Language> for LanguageRecord {
    fn from(domain: Language) -> Self {
        match domain {
            Language::De => LanguageRecord::De,
            Language::En => LanguageRecord::En,
            Language::Fr => LanguageRecord::Fr,
            Language::Es => LanguageRecord::Es,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Debug)]
pub struct TextRecord {
    pub text: String,
    pub language: LanguageRecord,
}

impl TextRecord {
    pub fn new(text: impl Into<String>, language: LanguageRecord) -> TextRecord {
        TextRecord {
            text: text.into(),
            language,
        }
    }
}

impl<T: Into<String>> From<Localized<Language, T>> for TextRecord {
    fn from(value: Localized<Language, T>) -> Self {
        TextRecord {
            text: value.payload.into(),
            language: value.localization.into(),
        }
    }
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

use crate::language::domain::Language;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ISO 639-1
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LanguageData {
    De,
    En,
    Fr,
    Es,
}

impl From<Language> for LanguageData {
    fn from(domain: Language) -> Self {
        match domain {
            Language::De => LanguageData::De,
            Language::En => LanguageData::En,
            Language::Fr => LanguageData::Fr,
            Language::Es => LanguageData::Es,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LocalizedTextData {
    pub text: String,
    pub language: LanguageData,
}

impl LocalizedTextData {
    pub fn new(text: String, language: LanguageData) -> Self {
        LocalizedTextData { text, language }
    }

    pub fn from_domain_fallbacked(
        domain: &HashMap<Language, String>,
        language: Language,
    ) -> Option<Self> {
        domain
            .get(&language)
            .map(|text| LocalizedTextData::new(text.to_owned(), language.into()))
            .or(domain
                .get(&Language::En)
                .map(|text| LocalizedTextData::new(text.to_owned(), LanguageData::En)))
            .or(domain
                .get(&Language::De)
                .map(|text| LocalizedTextData::new(text.to_owned(), LanguageData::De)))
            .or(domain
                .iter()
                .next()
                .map(|(lang, text)| LocalizedTextData::new(text.to_owned(), (*lang).into())))
    }
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

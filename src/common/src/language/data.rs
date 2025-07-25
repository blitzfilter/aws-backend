use crate::language::domain::Language;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ISO 639-1
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LanguageData {
    #[serde(
        alias = "de-DE",
        alias = "de-AT",
        alias = "de-CH",
        alias = "de-LU",
        alias = "de-LI"
    )]
    De,

    #[serde(
        alias = "en-US",
        alias = "en-GB",
        alias = "en-AU",
        alias = "en-CA",
        alias = "en-NZ",
        alias = "en_IE"
    )]
    En,

    #[serde(
        alias = "fr-FR",
        alias = "fr-CA",
        alias = "fr-BE",
        alias = "fr-CH",
        alias = "fr-LU"
    )]
    Fr,

    #[serde(
        alias = "es-ES",
        alias = "es-MX",
        alias = "es-AR",
        alias = "es-CO",
        alias = "es-CL",
        alias = "es-PE",
        alias = "es-VE"
    )]
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
        languages: &[Language],
    ) -> Option<Self> {
        languages
            .iter()
            .find_map(|lang| {
                domain
                    .get(lang)
                    .map(|text| LocalizedTextData::new(text.to_owned(), (*lang).into()))
            })
            .or_else(|| {
                domain
                    .get(&Language::De)
                    .map(|text| LocalizedTextData::new(text.to_owned(), LanguageData::De))
            })
            .or_else(|| {
                domain
                    .get(&Language::En)
                    .map(|text| LocalizedTextData::new(text.to_owned(), LanguageData::En))
            })
            .or_else(|| {
                domain.iter().next().map(|(lang, text)| {
                    LocalizedTextData::new(text.to_owned(), LanguageData::from(*lang))
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{LanguageData, LocalizedTextData};
    use crate::language::domain::Language;
    use rstest::rstest;
    use std::collections::HashMap;

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

    #[rstest::rstest]
    #[case::empty_defaults_german(&[], Some("German text".into()))]
    #[case::takes_preferred_from_singleton(&[Language::En], Some("English text".into()))]
    #[case::takes_preferred_from_many(&[Language::Es, Language::Fr, Language::En], Some("Spanish text".into()))]
    fn should_respect_language_priority_when_contains_all_for_from_domain_fallbacked(
        #[case] languages: &[Language],
        #[case] expected: Option<String>,
    ) {
        let domain = HashMap::from([
            (Language::De, "German text".to_owned()),
            (Language::En, "English text".to_owned()),
            (Language::Fr, "French text".to_owned()),
            (Language::Es, "Spanish text".to_owned()),
        ]);

        let actual = LocalizedTextData::from_domain_fallbacked(&domain, languages)
            .map(|localized_text_data| localized_text_data.text);

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case::empty_defaults_german(&[], Some("English text".into()))]
    #[case::takes_preferred_from_singleton(&[Language::En], Some("English text".into()))]
    #[case::takes_preferred_from_many(&[Language::Es, Language::Fr, Language::En], Some("French text".into()))]
    fn should_respect_language_priority_when_contains_some_for_from_domain_fallbacked(
        #[case] languages: &[Language],
        #[case] expected: Option<String>,
    ) {
        let domain = HashMap::from([
            (Language::En, "English text".to_owned()),
            (Language::Fr, "French text".to_owned()),
        ]);

        let actual = LocalizedTextData::from_domain_fallbacked(&domain, languages)
            .map(|localized_text_data| localized_text_data.text);

        assert_eq!(expected, actual);
    }

    #[rstest::rstest]
    #[case::empty_defaults_german(&[], Some("French text".into()))]
    #[case::takes_preferred_from_singleton(&[Language::En], Some("French text".into()))]
    #[case::takes_preferred_from_many(&[Language::Es, Language::En], Some("French text".into()))]
    fn should_resort_to_next_best_when_contains_no_match_nor_defaults_for_from_domain_fallbacked(
        #[case] languages: &[Language],
        #[case] expected: Option<String>,
    ) {
        let domain = HashMap::from([(Language::Fr, "French text".to_owned())]);

        let actual = LocalizedTextData::from_domain_fallbacked(&domain, languages)
            .map(|localized_text_data| localized_text_data.text);

        assert_eq!(expected, actual);
    }
}

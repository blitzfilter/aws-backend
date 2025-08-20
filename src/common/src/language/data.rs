use crate::{language::domain::Language, localized::Localized};
use serde::{Deserialize, Serialize};

// ISO 639-1
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Serialize, Deserialize, Copy, Clone, Eq, PartialEq, Debug, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum LanguageData {
    #[serde(
        alias = "de-DE",
        alias = "de-AT",
        alias = "de-CH",
        alias = "de-LU",
        alias = "de-LI"
    )]
    #[default]
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
    pub fn new(text: impl Into<String>, language: LanguageData) -> Self {
        LocalizedTextData {
            text: text.into(),
            language,
        }
    }
}

impl<T: Into<String>> From<Localized<Language, T>> for LocalizedTextData {
    fn from(value: Localized<Language, T>) -> Self {
        LocalizedTextData {
            text: value.payload.into(),
            language: value.localization.into(),
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{error::ApiError, error_code::BAD_HEADER_VALUE},
        language::data::LanguageData,
    };
    use http::{HeaderMap, HeaderValue, header::ACCEPT_LANGUAGE};

    pub fn extract_languages_header(headers: &HeaderMap) -> Result<Vec<LanguageData>, ApiError> {
        let languages = headers
            .get(ACCEPT_LANGUAGE)
            .map(HeaderValue::to_str)
            .map(|header_value_res| {
                header_value_res.map_err(|_| {
                    ApiError::bad_request(BAD_HEADER_VALUE)
                        .with_header_field(ACCEPT_LANGUAGE.as_str())
                })
            })
            .transpose()?
            .map(accept_language::parse)
            .unwrap_or_default()
            .into_iter()
            .map(|accept_language| {
                serde_json::from_str::<LanguageData>(&format!(r#""{accept_language}""#))
            })
            .filter_map(Result::ok)
            .collect::<Vec<_>>();

        Ok(languages)
    }

    pub fn extract_language_header(headers: &HeaderMap) -> Result<LanguageData, ApiError> {
        let language = extract_languages_header(headers)?
            .into_iter()
            .next()
            .unwrap_or_default();

        Ok(language)
    }

    #[cfg(test)]
    mod tests {
        use crate::language::data::LanguageData::{self, *};
        use crate::language::data::api::{extract_language_header, extract_languages_header};
        use http::HeaderMap;
        use http::header::ACCEPT_LANGUAGE;

        #[tokio::test]
        #[rstest::rstest]
        #[case("de", &[De])]
        #[case("de-DE", &[De])]
        #[case("en", &[En])]
        #[case("en-US", &[En])]
        #[case("en-GB", &[En])]
        #[case("es", &[Es])]
        #[case("es-ES", &[Es])]
        #[case("de;q=0.9,en;q=0.8", &[De, En])]
        #[case("en-GB,en;q=0.7,de;q=0.6", &[En, En, De])]
        #[case("es-ES;q=0.9,en;q=0.8,de;q=0.7", &[Es, En, De])]
        #[case("en,fr;q=0.5,de;q=0.3,es;q=0.2", &[En, Fr, De, Es])]
        #[case("pt-BR", &[])]
        #[case("ru", &[])]
        #[case("ja", &[])]
        #[case("zh-CN", &[])]
        #[case("ko-KR", &[])]
        #[case("*", &[])]
        #[case("fr-FR; q=0", &[Fr])]
        #[case("", &[])]
        #[case("null", &[])]
        #[case("undefined", &[])]
        #[case("\"en-US\"", &[])]
        #[case("123", &[])]
        #[case("abcdefg", &[])]
        async fn should_extract_languages(
            #[case] accept_language_header_value: &str,
            #[case] expected: &[LanguageData],
        ) {
            let mut header_map = HeaderMap::new();
            header_map.insert(
                ACCEPT_LANGUAGE,
                accept_language_header_value.try_into().unwrap(),
            );

            let actual = extract_languages_header(&header_map).unwrap();

            assert_eq!(expected, actual.as_slice())
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case("de", De)]
        #[case("de-DE", De)]
        #[case("en", En)]
        #[case("en-US", En)]
        #[case("en-GB", En)]
        #[case("es", Es)]
        #[case("es-ES", Es)]
        #[case("de;q=0.9,en;q=0.8", De)]
        #[case("en-GB,en;q=0.7,de;q=0.6", En)]
        #[case("es-ES;q=0.9,en;q=0.8,de;q=0.7", Es)]
        #[case("en,fr;q=0.5,de;q=0.3,es;q=0.2", En)]
        #[case("pt-BR", De)]
        #[case("ru", De)]
        #[case("ja", De)]
        #[case("zh-CN", De)]
        #[case("ko-KR", De)]
        #[case("*", De)]
        #[case("fr-FR; q=0", Fr)]
        #[case("", De)]
        #[case("null", De)]
        #[case("undefined", De)]
        #[case("\"en-US\"", De)]
        #[case("123", De)]
        #[case("abcdefg", De)]
        async fn should_extract_language(
            #[case] accept_language_header_value: &str,
            #[case] expected: LanguageData,
        ) {
            let mut header_map = HeaderMap::new();
            header_map.insert(
                ACCEPT_LANGUAGE,
                accept_language_header_value.try_into().unwrap(),
            );

            let actual = extract_language_header(&header_map).unwrap();

            assert_eq!(expected, actual)
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use crate::language::data::LocalizedTextData;
    use fake::{Dummy, Fake, Faker, Rng};

    impl Dummy<Faker> for LocalizedTextData {
        fn dummy_with_rng<R: Rng + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            LocalizedTextData {
                text: fake::faker::lorem::en::Sentence(5..20).fake_with_rng(rng),
                language: config.fake_with_rng(rng),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::language::data::LocalizedTextData;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_localized_text_data() {
            let _ = Faker.fake::<LocalizedTextData>();
        }
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

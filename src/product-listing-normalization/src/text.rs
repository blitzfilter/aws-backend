use crate::{error::NormalizationError, language::detect_language};
use localization::{Language, Localized};
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::{description::Description, title::Title};
use sha2::{Digest, Sha256};
use url::Url;

/// Strict variant used only in tests — returns an error when `raw` is blank
/// rather than falling back to the URL.
pub fn normalize_source_listing_id(raw: &str) -> Result<SourceListingId, NormalizationError> {
    SourceListingId::try_from(raw).map_err(|error| match error {
        product_listing_core::source_listing_id::InvalidSourceListingId::Blank => {
            NormalizationError::SourceListingIdEmpty
        }
        error => NormalizationError::SourceListingIdInvalid(error),
    })
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Returns the normalized extracted ID when present; otherwise falls back to
/// a SHA-256 hash of the full URL string.
pub fn normalize_source_listing_id_with_url_sha_fallback(
    raw: &str,
    url: &Url,
) -> Result<SourceListingId, NormalizationError> {
    if raw.trim().is_empty() {
        SourceListingId::try_from(sha256_hex(url.as_str()))
            .map_err(NormalizationError::SourceListingIdInvalid)
    } else {
        SourceListingId::try_from(raw).map_err(NormalizationError::SourceListingIdInvalid)
    }
}

/// Returns the trimmed title string, or [`NormalizationError::TitleEmpty`] if
/// the result is blank.
pub fn normalize_title(raw: &str) -> Result<Title, NormalizationError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(NormalizationError::TitleEmpty);
    }
    Ok(Title::from(trimmed))
}

/// Returns a localized title with language detection applied.
///
/// Errors with [`NormalizationError::TitleEmpty`] if the trimmed value is
/// blank, or [`NormalizationError::TitleUnknownLanguage`] if the language
/// cannot be detected.
#[allow(dead_code)]
pub fn normalize_title_localized(
    raw: &str,
) -> Result<Localized<Language, Title>, NormalizationError> {
    let title = normalize_title(raw)?;
    let title_language = detect_language(title.as_ref());
    localize_normalized_title(title, title_language, None)
}

fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

pub fn detect_description_language(fragments: &[String]) -> Option<Language> {
    let cleaned: Vec<String> = fragments
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    detect_language(cleaned.join("\n\n").as_str())
}

pub fn localize_normalized_title(
    title: Title,
    title_language: Option<Language>,
    description_language: Option<Language>,
) -> Result<Localized<Language, Title>, NormalizationError> {
    let language = if word_count(title.as_ref()) < 3 {
        description_language.or(title_language)
    } else {
        title_language
    }
    .ok_or_else(|| NormalizationError::TitleUnknownLanguage {
        text: title.as_ref().chars().take(100).collect(),
    })?;

    Ok(Localized::new(language, title))
}

/// Joins non-blank fragments with `\n\n`, detects the language of the
/// resulting text, and returns a localized description.
///
/// Returns `Ok(None)` when all fragments are blank.  Returns
/// [`NormalizationError::DescriptionUnknownLanguage`] when language detection
/// fails on the joined text and no fallback language is available.
pub fn normalize_description(
    fragments: Vec<String>,
    fallback_language: Option<Language>,
) -> Result<Option<Localized<Language, Description>>, NormalizationError> {
    let cleaned: Vec<String> = fragments
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();

    if cleaned.is_empty() {
        return Ok(None);
    }

    let joined = cleaned.join("\n\n");
    let description = Description::from(joined.as_str());
    let language = detect_language(description.as_ref())
        .or(fallback_language)
        .ok_or_else(|| NormalizationError::DescriptionUnknownLanguage {
            text: description.as_ref().chars().take(100).collect(),
        })?;

    Ok(Some(Localized::new(language, description)))
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use url::Url;

    use super::{
        detect_description_language, detect_language, localize_normalized_title,
        normalize_description, normalize_source_listing_id,
        normalize_source_listing_id_with_url_sha_fallback, normalize_title,
        normalize_title_localized,
    };
    use crate::error::NormalizationError;

    // -----------------------------------------------------------------------
    // source_listing_id
    // -----------------------------------------------------------------------

    #[test]
    fn should_preserve_case_when_source_listing_id_is_provided() {
        let result = normalize_source_listing_id("PROD-001").unwrap();
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_trim_outer_whitespace_when_normalizing_source_listing_id() {
        let result = normalize_source_listing_id("  PROD-001  ").unwrap();
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_return_error_when_source_listing_id_is_empty() {
        let err = normalize_source_listing_id("").unwrap_err();
        assert!(matches!(err, NormalizationError::SourceListingIdEmpty));
    }

    #[test]
    fn should_return_error_when_source_listing_id_is_only_whitespace() {
        let err = normalize_source_listing_id("   ").unwrap_err();
        assert!(matches!(err, NormalizationError::SourceListingIdEmpty));
    }

    // -----------------------------------------------------------------------
    // source_listing_id — URL SHA fallback
    // -----------------------------------------------------------------------

    #[test]
    fn should_use_extracted_id_when_source_listing_id_with_sha_fallback_is_non_empty() {
        let url = Url::parse("https://example.com/products/123").unwrap();
        let result = normalize_source_listing_id_with_url_sha_fallback("PROD-001", &url)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_trim_and_use_extracted_id_when_source_listing_id_with_sha_fallback_has_whitespace() {
        let url = Url::parse("https://example.com/products/123").unwrap();
        let result = normalize_source_listing_id_with_url_sha_fallback("  PROD-001  ", &url)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
        assert_eq!(result.to_string(), "PROD-001");
    }

    #[test]
    fn should_fall_back_to_sha256_of_url_when_source_listing_id_with_sha_fallback_is_empty() {
        let url = Url::parse("https://example.com/products/123").unwrap();
        let result = normalize_source_listing_id_with_url_sha_fallback("", &url)
            .unwrap_or_else(|error| panic!("valid fallback source listing ID: {error}"));
        assert_eq!(
            result.to_string(),
            "28c714c9f68ec26408de2fcdb45ef93e77920c3ef602cb85d57f9cd8fe5ea651"
        );
    }

    #[test]
    fn should_fall_back_to_sha256_of_url_when_source_listing_id_with_sha_fallback_is_only_whitespace()
     {
        let url = Url::parse("https://example.com/products/123").unwrap();
        let result = normalize_source_listing_id_with_url_sha_fallback("   ", &url)
            .unwrap_or_else(|error| panic!("valid fallback source listing ID: {error}"));
        assert_eq!(
            result.to_string(),
            "28c714c9f68ec26408de2fcdb45ef93e77920c3ef602cb85d57f9cd8fe5ea651"
        );
    }

    // -----------------------------------------------------------------------
    // title (raw — no language detection)
    // -----------------------------------------------------------------------

    #[test]
    fn should_normalize_title_when_plain_string_provided() {
        // normalize_title only trims and capitalises — no language detection.
        let title = normalize_title("Antique Vase").unwrap();
        assert_eq!(title.as_ref(), "Antique Vase");
    }

    #[test]
    fn should_capitalize_first_letter_when_title_starts_lowercase() {
        let title = normalize_title("antique vase").unwrap();
        assert_eq!(&title.as_ref()[..1], "A");
    }

    #[test]
    fn should_trim_whitespace_when_normalizing_title() {
        let title = normalize_title("  Antique Vase  ").unwrap();
        assert_eq!(title.as_ref(), "Antique Vase");
    }

    #[test]
    fn should_return_error_when_title_is_empty() {
        let err = normalize_title("").unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    #[test]
    fn should_return_error_when_title_is_only_whitespace() {
        let err = normalize_title("   ").unwrap_err();
        assert!(matches!(err, NormalizationError::TitleEmpty));
    }

    // -----------------------------------------------------------------------
    // title (localized — includes language detection)
    // -----------------------------------------------------------------------

    #[test]
    fn should_detect_language_for_title_when_english_text() {
        use localization::Language;
        let localized = normalize_title_localized("This is an antique vase from England").unwrap();
        assert_eq!(localized.localization, Language::En);
    }

    #[test]
    fn should_return_error_when_title_language_cannot_be_detected() {
        // Pure digits have no language signal — lingua returns None.
        let err = normalize_title_localized("12345").unwrap_err();
        assert!(
            matches!(err, NormalizationError::TitleUnknownLanguage { .. }),
            "expected TitleUnknownLanguage, got: {:?}",
            err
        );
    }

    #[test]
    fn should_use_description_language_for_short_title_when_available() {
        use localization::Language;
        let title = normalize_title("La Saintongeoise").unwrap();
        let title_language = detect_language(title.as_ref());
        let localized =
            localize_normalized_title(title, title_language, Some(Language::En)).unwrap();
        assert_eq!(localized.localization, Language::En);
    }

    #[test]
    fn should_fallback_to_title_detection_when_description_language_missing_for_short_title() {
        use localization::Language;
        let title = normalize_title("Vintage Poster").unwrap();
        let title_language = detect_language(title.as_ref());
        let localized = localize_normalized_title(title, title_language, None).unwrap();
        assert_eq!(localized.localization, Language::En);
    }

    #[test]
    fn should_detect_description_language_when_long_description_provided() {
        use localization::Language;
        let language = detect_description_language(&[
            "This vintage poster comes from a private collection and has documented ownership history."
                .to_string(),
        ]);
        assert_eq!(language, Some(Language::En));
    }

    // -----------------------------------------------------------------------
    // description
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_none_when_description_fragments_are_empty() {
        let result = normalize_description(vec![], None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn should_return_none_when_all_fragments_are_blank() {
        let result = normalize_description(vec!["  ".into(), "\t".into()], None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn should_join_fragments_with_double_newline_when_multiple_fragments() {
        let result = normalize_description(
            vec![
                "This antique piece comes from a private English collection.".into(),
                "It was acquired during the early twentieth century by the original owner.".into(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the original owner."
        );
    }

    #[test]
    fn should_trim_each_fragment_when_fragments_have_surrounding_whitespace() {
        let result = normalize_description(
            vec![
                "  This antique piece comes from a private English collection.  ".into(),
                "  It was acquired during the early twentieth century by the owner.  ".into(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the owner."
        );
    }

    #[test]
    fn should_skip_blank_fragments_when_some_fragments_are_blank() {
        let result = normalize_description(
            vec![
                "This antique piece comes from a private English collection.".into(),
                "  ".into(),
                "It was acquired during the early twentieth century by the original owner.".into(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection.\n\nIt was acquired during the early twentieth century by the original owner."
        );
    }

    #[test]
    fn should_return_single_paragraph_when_only_one_non_blank_fragment() {
        let result = normalize_description(
            vec![
                "This antique piece comes from a private English collection and dates to around nineteen twenty.".into(),
            ],
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.payload.as_ref(),
            "This antique piece comes from a private English collection and dates to around nineteen twenty."
        );
    }

    #[rstest]
    #[case(vec!["12345".into()])]
    fn should_return_error_when_description_language_cannot_be_detected(
        #[case] fragments: Vec<String>,
    ) {
        let err = normalize_description(fragments, None).unwrap_err();
        assert!(
            matches!(err, NormalizationError::DescriptionUnknownLanguage { .. }),
            "expected DescriptionUnknownLanguage, got: {:?}",
            err
        );
    }

    #[test]
    fn should_use_fallback_language_when_description_language_cannot_be_detected() {
        use localization::Language;

        let result = normalize_description(vec!["23-1/2\"18-1/4\"".into()], Some(Language::En))
            .unwrap()
            .unwrap();

        assert_eq!(result.localization, Language::En);
        assert_eq!(result.payload.as_ref(), "23-1/2\"18-1/4\"");
    }
}

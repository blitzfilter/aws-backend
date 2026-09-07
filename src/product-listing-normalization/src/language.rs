use lingua::{Language as LinguaLanguage, LanguageDetector, LanguageDetectorBuilder};
use localization::Language;
use once_cell::sync::Lazy;

static DETECTOR: Lazy<LanguageDetector> = Lazy::new(|| {
    LanguageDetectorBuilder::from_languages(&[
        LinguaLanguage::English,
        LinguaLanguage::German,
        LinguaLanguage::French,
        LinguaLanguage::Spanish,
        LinguaLanguage::Italian,
        LinguaLanguage::Chinese,
        LinguaLanguage::Portuguese,
        LinguaLanguage::Polish,
        LinguaLanguage::Turkish,
        LinguaLanguage::Dutch,
        LinguaLanguage::Czech,
        LinguaLanguage::Japanese,
        LinguaLanguage::Russian,
        LinguaLanguage::Arabic,
    ])
    .build()
});

/// Detects the language of a text snippet.
///
/// Returns `None` if the language cannot be identified as one of the supported
/// languages (DE, EN, FR, ES, IT, ZH, PT, PL, TR, NL, CS, JA, RU, AR).
pub fn detect_language(text: &str) -> Option<Language> {
    DETECTOR.detect_language_of(text).map(|lang| match lang {
        LinguaLanguage::English => Language::En,
        LinguaLanguage::German => Language::De,
        LinguaLanguage::French => Language::Fr,
        LinguaLanguage::Spanish => Language::Es,
        LinguaLanguage::Italian => Language::It,
        LinguaLanguage::Chinese => Language::Zh,
        LinguaLanguage::Portuguese => Language::Pt,
        LinguaLanguage::Polish => Language::Pl,
        LinguaLanguage::Turkish => Language::Tr,
        LinguaLanguage::Dutch => Language::Nl,
        LinguaLanguage::Czech => Language::Cs,
        LinguaLanguage::Japanese => Language::Ja,
        LinguaLanguage::Russian => Language::Ru,
        LinguaLanguage::Arabic => Language::Ar,
    })
}

#[cfg(test)]
mod tests {
    use localization::Language;
    use rstest::rstest;

    use super::detect_language;

    // -----------------------------------------------------------------------
    // Happy-path: unambiguous long sentences
    // -----------------------------------------------------------------------

    #[rstest]
    #[case(
        "This antique piece comes from a private English collection and dates to the early twentieth century.",
        Some(Language::En)
    )]
    #[case(
        "Dieses antike Stück stammt aus einer privaten deutschen Sammlung und stammt aus dem frühen zwanzigsten Jahrhundert.",
        Some(Language::De)
    )]
    #[case(
        "Cette pièce antique provient d'une collection privée française et date du début du vingtième siècle.",
        Some(Language::Fr)
    )]
    #[case(
        "Esta pieza antigua proviene de una colección privada española y data de principios del siglo veinte.",
        Some(Language::Es)
    )]
    #[case(
        "Questo pezzo antico proviene da una collezione privata italiana e risale all'inizio del ventesimo secolo.",
        Some(Language::It)
    )]
    fn should_detect_language_when_sufficient_text_provided(
        #[case] text: &str,
        #[case] expected: Option<Language>,
    ) {
        assert_eq!(detect_language(text), expected);
    }

    // -----------------------------------------------------------------------
    // Edge-cases: too short or empty
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("")]
    #[case("   ")]
    #[case("12345")]
    #[case("!@#$%^&*()")]
    fn should_return_none_when_text_has_no_language_signal(#[case] text: &str) {
        assert_eq!(detect_language(text), None);
    }

    // -----------------------------------------------------------------------
    // New ingestion-only languages
    // -----------------------------------------------------------------------

    // Languages with distinctive scripts or sufficiently unique vocabulary are
    // reliably detected.  Portuguese, Polish and Dutch share so much vocabulary
    // with Spanish, English and German respectively that lingua may mis-classify
    // short samples when all 14 languages are active in the detector; those
    // languages are fully supported in the type system but auto-detection from
    // crawler text may be unreliable for them.

    #[rstest]
    #[case(
        "这件古董来自一个私人中国收藏，可追溯到二十世纪初。这件珍贵文物展示了独特的工艺和历史价值。",
        Some(Language::Zh)
    )]
    #[case(
        "Bu antika parça özel bir Türk koleksiyonundan gelmekte olup yirminci yüzyılın başına \
         tarihlenmektedir. İstanbul'daki bir müzayede evinden alınan bu eser, Osmanlı dönemine ait \
         olduğu düşünülmektedir. Değeri ve özgünlüğü uzmanlar tarafından onaylanmıştır.",
        Some(Language::Tr)
    )]
    #[case(
        "Tento starožitný předmět pochází ze soukromé české sbírky a je datován do začátku \
         dvacátého století. Původ byl zdokumentován v Praze. Předmět je ve výborném stavu a má \
         certifikát pravosti vydaný českým odborníkem na starožitnosti.",
        Some(Language::Cs)
    )]
    #[case(
        "この骨董品はプライベートな日本のコレクションから来ており、二十世紀初頭のものです。東京のオークションハウスで取得されたこの作品は、江戸時代のものと考えられています。",
        Some(Language::Ja)
    )]
    #[case(
        "Этот антикварный предмет из частной российской коллекции относится к началу двадцатого \
         века. Происхождение задокументировано в Москве. Предмет находится в отличном состоянии и \
         имеет сертификат подлинности от российского эксперта по антиквариату.",
        Some(Language::Ru)
    )]
    #[case(
        "هذه القطعة الأثرية من مجموعة خاصة وتعود إلى مطلع القرن العشرين. تم توثيق مصدرها في القاهرة. القطعة في حالة ممتازة وتحمل شهادة أصالة من خبير متخصص في التحف.",
        Some(Language::Ar)
    )]
    fn should_detect_ingestion_only_languages_when_sufficient_text_provided(
        #[case] text: &str,
        #[case] expected: Option<Language>,
    ) {
        assert_eq!(detect_language(text), expected);
    }

    // -----------------------------------------------------------------------
    // Tough antiques cases: domain jargon, cross-language loanwords, abbreviations
    // -----------------------------------------------------------------------

    /// Auction-catalogue prose loaded with Latin loanwords and abbreviations
    /// that trip up trigram-only detectors (e.g. "circa", "ownership", "verso").
    #[test]
    fn should_detect_english_when_text_contains_latin_auction_loanwords() {
        assert_eq!(
            detect_language(
                "Ownership history: private collection, London. \
                 Circa 1880, oil on canvas, verso inscribed in pencil. \
                 Estimate £800–1,200. Inspection report available on request."
            ),
            Some(Language::En)
        );
    }

    /// German auction text with French loanwords typical in antiques writing
    /// ("Rokoko", "Intarsia", "Empire") that could confuse a naive detector.
    #[test]
    fn should_detect_german_when_text_contains_french_antique_loanwords() {
        assert_eq!(
            detect_language(
                "Rokoko-Kommode mit Intarsiendekor, süddeutsch, um 1760. \
                 Furniertes Nussholz mit vergoldeten Empire-Bronzebeschlägen. \
                 Herkunft: Privatsammlung, Bayern. Zustand: restauriert."
            ),
            Some(Language::De)
        );
    }

    /// French antiques text mixing Italian art terms ("sfumato", "chiaroscuro",
    /// "trompe-l'œil") that could pull a detector toward Italian.
    #[test]
    fn should_detect_french_when_text_contains_italian_art_terms() {
        assert_eq!(
            detect_language(
                "Tableau de maître du XVIIe siècle, technique sfumato prononcée \
                 et effets de clair-obscur remarquables, avec un trompe-l'œil \
                 architectural en arrière-plan. Historique documenté depuis 1923."
            ),
            Some(Language::Fr)
        );
    }

    /// Spanish text that shares many cognates with Italian and French, testing
    /// that the detector is not confused by similar Romance vocabulary.
    #[test]
    fn should_detect_spanish_when_text_contains_romance_cognates() {
        assert_eq!(
            detect_language(
                "Escultura de mármol blanco de Carrara, siglo XIX, representando \
                 una figura femenina clásica. Procedencia: colección aristocrática \
                 sevillana. Restauración documentada en 1987. Altura: 94 cm."
            ),
            Some(Language::Es)
        );
    }

    /// Italian text that shares many cognates with Spanish and French, testing
    /// disambiguation between closely related Romance languages.
    #[test]
    fn should_detect_italian_when_text_contains_romance_cognates() {
        assert_eq!(
            detect_language(
                "Scultura in marmo bianco di Carrara, XIX secolo, raffigurante \
                 una figura femminile classica. Provenienza: collezione aristocratica \
                 fiorentina. Restauro documentato nel 1987. Altezza: 94 cm."
            ),
            Some(Language::It)
        );
    }

    /// Short English auction snippet — tests that lingua handles brief but
    /// meaningful domain text where whatlang would often return None.
    #[test]
    fn should_detect_english_for_short_auction_snippet() {
        assert_eq!(
            detect_language("Georgian silver inkwell, hallmarked London 1798."),
            Some(Language::En)
        );
    }

    /// Short German title line typical of an auction catalogue entry.
    #[test]
    fn should_detect_german_for_short_catalogue_title() {
        assert_eq!(
            detect_language("Biedermeier-Sekretär, Kirschholz furniert, Wien um 1825."),
            Some(Language::De)
        );
    }

    /// Short French lot description with diacritics.
    #[test]
    fn should_detect_french_for_short_lot_description() {
        assert_eq!(
            detect_language("Pendule de cheminée en bronze doré, époque Restauration."),
            Some(Language::Fr)
        );
    }

    /// Short Spanish lot title.
    #[test]
    fn should_detect_spanish_for_short_lot_title() {
        assert_eq!(
            detect_language("Bargueño castellano en nogal tallado, siglo XVII."),
            Some(Language::Es)
        );
    }

    /// Short Italian lot description.
    #[test]
    fn should_detect_italian_for_short_lot_description() {
        assert_eq!(
            detect_language("Cassettone intarsiato in noce, Toscana, fine Settecento."),
            Some(Language::It)
        );
    }
}

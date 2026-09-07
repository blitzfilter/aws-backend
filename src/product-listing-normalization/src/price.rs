use money::{Currency, HasMinorUnitExponent, MonetaryAmount, Price};
use regex::regex;

// ---------------------------------------------------------------------------
// Internal error type
// ---------------------------------------------------------------------------

/// Internal error for price parsing — carries no field context yet.
/// Callers map this to the appropriate [`NormalizationError`] variant.
#[derive(Debug, PartialEq)]
pub enum PriceNormalizationError {
    /// No recognised currency symbol or ISO code was found in the string.
    UnknownCurrency,
    /// A currency was detected but the numeric amount could not be parsed.
    ParseFailure,
}

pub type PriceError = PriceNormalizationError;

// ---------------------------------------------------------------------------
// Currency detection
// ---------------------------------------------------------------------------

/// Detects the currency from a raw price string.
///
/// Returns `None` if no recognized currency symbol or ISO code is present.
/// Multi-character symbols (`NZD`, `AUD`, `CAD`) are checked before the plain
/// `$` to avoid false matches.
pub fn detect_currency(raw: &str) -> Option<Currency> {
    if raw.contains("NZD") || raw.contains("NZ$") {
        Some(Currency::Nzd)
    } else if raw.contains("AUD") || raw.contains("A$") {
        Some(Currency::Aud)
    } else if raw.contains("CAD") || raw.contains("C$") {
        Some(Currency::Cad)
    } else if raw.contains("USD") || raw.contains('$') {
        Some(Currency::Usd)
    } else if raw.contains("GBP") || raw.contains('£') {
        Some(Currency::Gbp)
    } else if raw.contains("EUR") || raw.contains('€') {
        Some(Currency::Eur)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Price parsing
// ---------------------------------------------------------------------------

/// Strips currency symbols / codes and converts the remaining string to a
/// [`MonetaryAmount`] + [`Currency`] pair.
///
/// Handles formats such as:
///   - `"1.234,56 €"`   (European: dot-thousands, comma-decimal)
///   - `"1,234.56 USD"` (Anglo: comma-thousands, dot-decimal)
///   - `"£8,800"`       (Anglo: comma-thousands, no decimal)
///   - `"£1 234.56"`    (space-thousands)
///   - `"6 900 €"`      (narrow no-break space-thousands)
///   - `"1234.5"`       (single decimal digit)
///   - `"1'234.56"`     (apostrophe-thousands)
///
/// If no currency marker is found in `raw` the optional `fallback_currency` is
/// used (e.g. inferred from the ListingSource domain TLD). If neither is present
/// [`PriceError::UnknownCurrency`] is returned.
pub fn parse_price(
    raw: &str,
    fallback_currency: Option<Currency>,
) -> Result<(MonetaryAmount, Currency), PriceNormalizationError> {
    let currency = detect_currency(raw)
        .or(fallback_currency)
        .ok_or(PriceError::UnknownCurrency)?;

    let number = extract_price_number_candidate(raw, &currency)?;

    let amount = parse_price_number(&number, &currency)?;

    Ok((amount, currency))
}

fn parse_price_number(number: &str, currency: &Currency) -> Result<MonetaryAmount, PriceError> {
    let exponent = minor_unit_exponent(currency);

    // Remove whitespace and apostrophes used as thousands separators.
    let stripped = number.replace([' ', '\u{00a0}', '\u{202f}', '\'', '_'], "");

    // Determine whether comma or dot is the decimal separator, then split.
    let (integer_part, fraction_part): (&str, &str) =
        if let Some((left, right)) = split_decimal(&stripped) {
            (left, right)
        } else {
            (stripped.as_str(), "0")
        };

    // Strip remaining thousands-separator characters from each part.
    let integer_clean: String = integer_part
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    let fraction_clean: String = fraction_part
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();

    if integer_clean.is_empty() && fraction_clean == "0" {
        return Err(PriceError::ParseFailure);
    }

    let fraction_normalised = normalise_fraction(&fraction_clean, exponent);

    let amount_str = format!("{}{}", integer_clean, fraction_normalised);
    let amount: u64 = amount_str.parse().map_err(|_| PriceError::ParseFailure)?;

    Ok(MonetaryAmount::from(amount))
}

// ---------------------------------------------------------------------------
// Public field-level helper
// ---------------------------------------------------------------------------

/// Parses an optional raw price string with explicit fallback currency context.
/// Blank values and deliberate price-on-request markers produce no assertion.
pub fn normalize_price(
    raw: Option<&str>,
    fallback_currency: Option<Currency>,
) -> Result<Option<Price>, PriceNormalizationError> {
    let Some(raw) = raw else { return Ok(None) };
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_price_on_request_marker(trimmed) {
        return Ok(None);
    }
    parse_price(trimmed, fallback_currency)
        .map(|(amount, currency)| Some(Price::new(amount, currency)))
}

/// Parses one machine-supplied decimal with no display-text interpretation.
///
/// The full value must be an unsigned ASCII decimal (`digits` or `digits.digits`). Extra
/// fractional precision is accepted only when it is trailing zero padding, so no nonzero value is
/// silently truncated.
pub(crate) fn normalize_machine_decimal_price(
    raw: &str,
    currency: Currency,
) -> Result<Price, PriceNormalizationError> {
    let (integer, fraction) = match raw.split_once('.') {
        Some((integer, fraction)) if !fraction.is_empty() && !fraction.contains('.') => {
            (integer, fraction)
        }
        Some(_) => return Err(PriceNormalizationError::ParseFailure),
        None => (raw, ""),
    };
    if integer.is_empty()
        || !integer.bytes().all(|digit| digit.is_ascii_digit())
        || !fraction.bytes().all(|digit| digit.is_ascii_digit())
    {
        return Err(PriceNormalizationError::ParseFailure);
    }

    let exponent = minor_unit_exponent(&currency) as usize;
    if fraction.len() > exponent && fraction[exponent..].bytes().any(|digit| digit != b'0') {
        return Err(PriceNormalizationError::ParseFailure);
    }

    let major = parse_machine_decimal_digits(integer)?;
    let minor_factor = 10_u64
        .checked_pow(exponent as u32)
        .ok_or(PriceNormalizationError::ParseFailure)?;
    let retained_fraction = &fraction[..fraction.len().min(exponent)];
    let fraction_digits = parse_machine_decimal_digits(retained_fraction)?;
    let fraction_scale = 10_u64
        .checked_pow((exponent - retained_fraction.len()) as u32)
        .ok_or(PriceNormalizationError::ParseFailure)?;
    let minor = major
        .checked_mul(minor_factor)
        .and_then(|amount| {
            fraction_digits
                .checked_mul(fraction_scale)
                .and_then(|fraction| amount.checked_add(fraction))
        })
        .ok_or(PriceNormalizationError::ParseFailure)?;

    Ok(Price::new(MonetaryAmount::from(minor), currency))
}

fn parse_machine_decimal_digits(digits: &str) -> Result<u64, PriceNormalizationError> {
    digits.bytes().try_fold(0_u64, |value, digit| {
        let digit = digit
            .checked_sub(b'0')
            .filter(|digit| *digit <= 9)
            .ok_or(PriceNormalizationError::ParseFailure)?;
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(digit)))
            .ok_or(PriceNormalizationError::ParseFailure)
    })
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Returns the number of minor-unit digits for the given currency (always 2
/// for the currencies we support).
fn minor_unit_exponent(currency: &Currency) -> u32 {
    currency.minor_unit_exponent().0 as u32
}

/// Pads with trailing zeros or truncates to exactly `exponent` digits.
fn normalise_fraction(frac: &str, exponent: u32) -> String {
    let exp = exponent as usize;
    if frac.len() >= exp {
        frac[..exp].to_owned()
    } else {
        format!("{:0<width$}", frac, width = exp)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PriceNumberCandidate {
    value: String,
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PriceNumberCandidateQuality {
    Clean,
    Malformed,
}

struct ParsedPriceNumberCandidate {
    candidate: PriceNumberCandidate,
    amount: MonetaryAmount,
    quality: PriceNumberCandidateQuality,
}

fn extract_price_number_candidate(raw: &str, currency: &Currency) -> Result<String, PriceError> {
    let candidates = price_like_number_candidates(raw);
    let mut parsed = Vec::new();

    for candidate in candidates {
        parsed.push(ParsedPriceNumberCandidate {
            amount: parse_price_number(&candidate.value, currency)?,
            quality: price_number_candidate_quality(&candidate.value),
            candidate,
        });
    }

    match parsed.as_slice() {
        [] => Err(PriceError::ParseFailure),
        [single] => Ok(single.candidate.value.clone()),
        _ => select_price_number_candidate(raw, parsed),
    }
}

fn price_like_number_candidates(raw: &str) -> Vec<PriceNumberCandidate> {
    let candidates = extract_price_number_candidates(raw);
    let currency_markers = currency_marker_spans(raw);

    if currency_markers.is_empty() {
        return candidates;
    }

    let currency_bearing_candidates: Vec<_> = candidates
        .iter()
        .filter(|candidate| {
            currency_markers.iter().any(|marker| {
                distance_between_spans((candidate.start, candidate.end), *marker) <= 2
            })
        })
        .cloned()
        .collect();

    if currency_bearing_candidates.is_empty() {
        candidates
    } else {
        currency_bearing_candidates
    }
}

fn select_price_number_candidate(
    raw: &str,
    candidates: Vec<ParsedPriceNumberCandidate>,
) -> Result<String, PriceError> {
    if candidates
        .iter()
        .all(|candidate| candidate.amount == candidates[0].amount)
    {
        return Ok(best_price_number_candidate(raw, &candidates)?.value.clone());
    }

    let clean_candidates: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.quality == PriceNumberCandidateQuality::Clean)
        .collect();
    let has_malformed_candidate = candidates
        .iter()
        .any(|candidate| candidate.quality == PriceNumberCandidateQuality::Malformed);

    if clean_candidates.len() == 1 && has_malformed_candidate {
        return Ok(clean_candidates[0].candidate.value.clone());
    }

    if clean_candidates.len() > 1 {
        return Err(PriceError::ParseFailure);
    }

    Ok(best_price_number_candidate(raw, &candidates)?.value.clone())
}

fn best_price_number_candidate<'a>(
    raw: &str,
    candidates: &'a [ParsedPriceNumberCandidate],
) -> Result<&'a PriceNumberCandidate, PriceError> {
    let currency_marker = first_currency_marker_span(raw);

    let best = match currency_marker {
        Some(marker) => candidates.iter().min_by_key(|candidate| {
            distance_between_spans((candidate.candidate.start, candidate.candidate.end), marker)
        }),
        None => candidates.first(),
    };

    best.map(|candidate| &candidate.candidate)
        .ok_or(PriceError::ParseFailure)
}

fn extract_price_number_candidates(raw: &str) -> Vec<PriceNumberCandidate> {
    let mut candidates = Vec::new();
    let mut start = None;
    let mut last_digit_end = 0usize;

    for (idx, ch) in raw.char_indices() {
        if ch.is_ascii_digit() {
            if start.is_none() {
                start = Some(idx);
            }
            last_digit_end = idx + ch.len_utf8();
        } else if start.is_some()
            && matches!(ch, '.' | ',' | '\'' | '_' | ' ' | '\u{00a0}' | '\u{202f}')
        {
            continue;
        } else if let Some(s) = start.take() {
            candidates.push(price_number_candidate(raw, s, last_digit_end));
        }
    }

    if let Some(s) = start {
        candidates.push(price_number_candidate(raw, s, last_digit_end));
    }

    candidates
}

fn price_number_candidate(raw: &str, start: usize, end: usize) -> PriceNumberCandidate {
    let trimmed = raw[start..end].trim();
    let leading_trimmed = raw[start..end].len() - raw[start..end].trim_start().len();
    let trailing_trimmed = raw[start..end].len() - raw[start..end].trim_end().len();

    PriceNumberCandidate {
        value: trimmed.to_string(),
        start: start + leading_trimmed,
        end: end - trailing_trimmed,
    }
}

fn price_number_candidate_quality(value: &str) -> PriceNumberCandidateQuality {
    let stripped = value.replace([' ', '\u{00a0}', '\u{202f}', '\'', '_'], "");
    let last_dot = stripped.rfind('.');
    let last_comma = stripped.rfind(',');
    let decimal_index = match (last_dot, last_comma) {
        (Some(dot), Some(comma)) => Some(dot.max(comma)),
        (Some(dot), None) => Some(dot),
        (None, Some(comma)) => Some(comma),
        (None, None) => None,
    };

    let Some(index) = decimal_index else {
        return PriceNumberCandidateQuality::Clean;
    };

    let trailing_digits = stripped[index + 1..]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .count();

    if trailing_digits > 3 {
        PriceNumberCandidateQuality::Malformed
    } else {
        PriceNumberCandidateQuality::Clean
    }
}

fn first_currency_marker_span(raw: &str) -> Option<(usize, usize)> {
    currency_marker_spans(raw).into_iter().next()
}

fn currency_marker_spans(raw: &str) -> Vec<(usize, usize)> {
    regex!(r"(?i)(EUR|USD|GBP|AUD|CAD|NZD|NZ\$|A\$|C\$|\$|\x{00A3}|\x{20AC})")
        .find_iter(raw)
        .map(|m| (m.start(), m.end()))
        .collect()
}

fn distance_between_spans(left: (usize, usize), right: (usize, usize)) -> usize {
    right
        .0
        .saturating_sub(left.1)
        .max(left.0.saturating_sub(right.1))
}

/// Returns `true` when the string slice is exactly 3 ASCII digits, which
/// indicates a thousands-separator group rather than a decimal fraction.
fn is_thousands_group(s: &str) -> bool {
    s.len() == 3 && s.chars().all(|c| c.is_ascii_digit())
}

/// Splits a cleaned number string at the decimal separator.
///
/// The *later* of the last `.` and last `,` is treated as the decimal
/// separator; the earlier one (if present) is the thousands separator.
///
/// Special case: when only a single separator is present and the trailing
/// group has **exactly 3 digits** it is treated as a thousands separator and
/// `None` is returned (e.g. `"8,800"` → `None`, `"8.800"` → `None`).
fn split_decimal(s: &str) -> Option<(&str, &str)> {
    let last_dot = s.rfind('.');
    let last_comma = s.rfind(',');

    match (last_dot, last_comma) {
        (None, None) => None,
        (Some(d), None) => {
            let after = &s[d + 1..];
            if is_thousands_group(after) {
                None
            } else {
                Some((&s[..d], after))
            }
        }
        (None, Some(c)) => {
            let after = &s[c + 1..];
            if is_thousands_group(after) {
                None
            } else {
                Some((&s[..c], after))
            }
        }
        (Some(d), Some(c)) => {
            if d > c {
                Some((&s[..d], &s[d + 1..]))
            } else {
                Some((&s[..c], &s[c + 1..]))
            }
        }
    }
}

fn is_price_on_request_marker(raw: &str) -> bool {
    // Keywords that, when present anywhere in the price string, indicate that
    // the seller intentionally has not set a price, and it must be requested.
    // Covers: EN, DE, FR, IT, ES, PT, NL, PL, RU, ZH, JA, AR
    const KEYWORDS: &[&str] = &[
        // English
        "request", // "on request", "price on request", "price upon request"
        "enquire", // "please enquire"
        "inquire", // "please inquire"
        "contact us",
        "call for price",
        "ask for price",
        "poa",
        // German
        "anfrage", // "auf Anfrage", "Preis auf Anfrage"
        // French
        "demande", // "sur demande", "prix sur demande"
        // Italian
        "richiesta", // "su richiesta", "prezzo su richiesta"
        // Spanish
        "consultar", // "precio a consultar"
        "bajo pedido",
        // Portuguese
        "consulte",     // "consulte-nos"
        "sob consulta", // "preço sob consulta"
        // Dutch
        "aanvraag", // "op aanvraag", "prijs op aanvraag"
        // Polish
        "zapytanie", // "na zapytanie", "cena na zapytanie"
        // Russian
        "по запросу", // "цена по запросу"
        // Chinese
        "询价",
        "面议",
        // Japanese
        "お問い合わせ", // "価格はお問い合わせ"
        // Arabic
        "بالتفاوض",
        "عند الطلب",
    ];

    let lower = raw.to_lowercase();
    KEYWORDS.iter().any(|kw| lower.contains(kw))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use money::Currency;

    use super::{
        PriceError, detect_currency, extract_price_number_candidate, is_price_on_request_marker,
        normalise_fraction, normalize_machine_decimal_price, parse_price, split_decimal,
    };

    // -----------------------------------------------------------------------
    // detect_currency
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("$100", Some(Currency::Usd))]
    #[case("USD 100", Some(Currency::Usd))]
    #[case("£100", Some(Currency::Gbp))]
    #[case("GBP 100", Some(Currency::Gbp))]
    #[case("€ 100", Some(Currency::Eur))]
    #[case("EUR 100", Some(Currency::Eur))]
    #[case("A$100", Some(Currency::Aud))]
    #[case("AUD 100", Some(Currency::Aud))]
    #[case("C$100", Some(Currency::Cad))]
    #[case("CAD 100", Some(Currency::Cad))]
    #[case("NZ$100", Some(Currency::Nzd))]
    #[case("NZD 100", Some(Currency::Nzd))]
    #[case("100", None)]
    #[case("CHF 100", None)]
    #[case("", None)]
    fn should_detect_currency_when_symbol_or_code_present(
        #[case] raw: &str,
        #[case] expected: Option<Currency>,
    ) {
        assert_eq!(detect_currency(raw), expected);
    }

    #[test]
    fn should_prefer_nzd_over_plain_dollar_when_nz_prefix_present() {
        assert_eq!(detect_currency("NZ$100"), Some(Currency::Nzd));
    }

    #[test]
    fn should_prefer_aud_over_plain_dollar_when_a_prefix_present() {
        assert_eq!(detect_currency("A$100"), Some(Currency::Aud));
    }

    #[test]
    fn should_prefer_cad_over_plain_dollar_when_c_prefix_present() {
        assert_eq!(detect_currency("C$100"), Some(Currency::Cad));
    }

    // -----------------------------------------------------------------------
    // parse_price — successful cases
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("1.234,56 €", 123456, Currency::Eur)]
    #[case("1,234.56 USD", 123456, Currency::Usd)]
    #[case("£1 234.56", 123456, Currency::Gbp)]
    #[case("6\u{202f}900\u{00a0}€", 690000, Currency::Eur)]
    #[case("100 EUR", 10000, Currency::Eur)]
    #[case("€ 50,00", 5000, Currency::Eur)]
    #[case("$9.99", 999, Currency::Usd)]
    #[case("A$12.50", 1250, Currency::Aud)]
    #[case("C$99.99", 9999, Currency::Cad)]
    #[case("NZ$25.00", 2500, Currency::Nzd)]
    #[case("GBP 1234", 123400, Currency::Gbp)]
    #[case("0.50 USD", 50, Currency::Usd)]
    #[case("1.5 EUR", 150, Currency::Eur)]
    #[case("1'234.56 USD", 123456, Currency::Usd)]
    #[case("£ 0.01", 1, Currency::Gbp)]
    // UK/US comma-thousands prices (the comma is a thousands separator, not decimal)
    #[case("£8,800", 880000, Currency::Gbp)]
    #[case("$1,500", 150000, Currency::Usd)]
    #[case("$10,000", 1000000, Currency::Usd)]
    #[case("£1,800,000", 180000000, Currency::Gbp)]
    #[case("Preis: \u{20ac} 680,00 inkl. MwSt.", 68000, Currency::Eur)]
    #[case("\u{20ac} 680,00 inkl. MwSt.", 68000, Currency::Eur)]
    #[case("Item 02092 Price: \u{20ac} 680,00", 68000, Currency::Eur)]
    #[case("Item 02092 Price: $1,125", 112500, Currency::Usd)]
    #[case("Art.-Nr. 02092 Preis 680,00 \u{20ac}", 68000, Currency::Eur)]
    #[case("02092 / GBP 1,234.56", 123456, Currency::Gbp)]
    #[case(
        "Regular price \u{20ac}11.32795 \u{20ac}11.327,95",
        1132795,
        Currency::Eur
    )]
    #[case("\u{00a3}3,90000\u{00a3}3,900.00", 390000, Currency::Gbp)]
    #[case("$3,900$3,900.00", 390000, Currency::Usd)]
    fn should_parse_price_when_valid_string_provided(
        #[case] raw: &str,
        #[case] expected_amount: u64,
        #[case] expected_currency: Currency,
    ) {
        let (amount, currency) = parse_price(raw, None).unwrap();
        assert_eq!(*amount, expected_amount, "amount mismatch for '{}'", raw);
        assert_eq!(
            currency, expected_currency,
            "currency mismatch for '{}'",
            raw
        );
    }

    // -----------------------------------------------------------------------
    // parse_price — error cases
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("no numbers here USD")]
    #[case("$100.00$80.00")]
    #[case("€")]
    fn should_return_parse_failure_when_price_has_currency_but_no_number(#[case] raw: &str) {
        assert!(
            matches!(parse_price(raw, None), Err(PriceError::ParseFailure)),
            "expected ParseFailure for '{}'",
            raw
        );
    }

    #[rstest]
    #[case("")]
    #[case("   ")]
    #[case("no numbers here")]
    #[case("1234.56 CHF")]
    fn should_return_unknown_currency_when_no_currency_symbol_or_known_code(#[case] raw: &str) {
        assert!(
            matches!(parse_price(raw, None), Err(PriceError::UnknownCurrency)),
            "expected UnknownCurrency for '{}'",
            raw
        );
    }

    // -----------------------------------------------------------------------
    // parse_price — fallback currency
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("18,00", Currency::Eur, 1800u64)]
    #[case("1590", Currency::Eur, 159000u64)]
    #[case("1590", Currency::Gbp, 159000u64)]
    #[case("1.234,56", Currency::Eur, 123456u64)]
    #[case("680,00 inkl. MwSt.", Currency::Eur, 68000u64)]
    fn should_parse_bare_price_when_fallback_currency_provided(
        #[case] raw: &str,
        #[case] fallback: Currency,
        #[case] expected_amount: u64,
    ) {
        let (amount, currency) = parse_price(raw, Some(fallback)).unwrap();
        assert_eq!(*amount, expected_amount, "amount mismatch for '{}'", raw);
        assert_eq!(currency, fallback, "currency mismatch for '{}'", raw);
    }

    #[rstest]
    #[case("42.000", Currency::Eur, 4_200_u64)]
    #[case("42.5", Currency::Eur, 4_250_u64)]
    #[case("42.50", Currency::Eur, 4_250_u64)]
    #[case("0", Currency::Eur, 0_u64)]
    #[case("42.050", Currency::Eur, 4_205_u64)]
    #[case("42.000", Currency::Jpy, 42_u64)]
    fn should_parse_machine_decimal_to_exact_minor_units(
        #[case] raw: &str,
        #[case] currency: Currency,
        #[case] expected_amount: u64,
    ) {
        let price = normalize_machine_decimal_price(raw, currency)
            .unwrap_or_else(|error| panic!("machine decimal should parse: {error:?}"));

        assert_eq!(currency, price.currency);
        assert_eq!(expected_amount, *price.monetary_amount);
    }

    #[rstest]
    #[case("")]
    #[case("42.")]
    #[case(".42")]
    #[case("+42")]
    #[case("-42")]
    #[case("42,00")]
    #[case("1,000")]
    #[case("1_000")]
    #[case("EUR 42")]
    #[case("42 EUR")]
    #[case("€42")]
    #[case("42e0")]
    #[case("42.001")]
    #[case(" 42")]
    #[case("42 ")]
    #[case("４２")]
    fn should_reject_noncanonical_machine_decimal_text(#[case] raw: &str) {
        assert_eq!(
            Err(PriceError::ParseFailure),
            normalize_machine_decimal_price(raw, Currency::Eur)
        );
    }

    #[test]
    fn should_reject_machine_decimal_nonzero_precision_for_zero_minor_unit_currency() {
        assert_eq!(
            Err(PriceError::ParseFailure),
            normalize_machine_decimal_price("42.5", Currency::Jpy)
        );
    }

    #[test]
    fn should_check_machine_decimal_minor_unit_overflow() {
        let maximum = normalize_machine_decimal_price("184467440737095516.15", Currency::Eur)
            .unwrap_or_else(|error| panic!("maximum minor unit value should parse: {error:?}"));
        assert_eq!(u64::MAX, *maximum.monetary_amount);
        assert_eq!(
            Err(PriceError::ParseFailure),
            normalize_machine_decimal_price("184467440737095516.16", Currency::Eur)
        );
    }

    #[rstest]
    #[case("Preis:  680,00 inkl. MwSt.", Currency::Eur, Some("680,00"))]
    #[case("  1 234.56 gross", Currency::Gbp, Some("1 234.56"))]
    #[case("6\u{202f}900\u{00a0}€", Currency::Eur, Some("6\u{202f}900"))]
    #[case("Item 02092 Price: \u{20ac} 680,00", Currency::Eur, Some("680,00"))]
    #[case("Item 02092 Price: $1,125", Currency::Usd, Some("1,125"))]
    #[case("Art.-Nr. 02092 Preis 680,00 \u{20ac}", Currency::Eur, Some("680,00"))]
    #[case("02092 / GBP 1,234.56", Currency::Gbp, Some("1,234.56"))]
    #[case(
        "Regular price \u{20ac}11.32795 \u{20ac}11.327,95",
        Currency::Eur,
        Some("11.327,95")
    )]
    #[case("no numbers here", Currency::Eur, None)]
    fn should_extract_price_number_candidate(
        #[case] raw: &str,
        #[case] currency: Currency,
        #[case] expected: Option<&str>,
    ) {
        match expected {
            Some(expected) => {
                let actual = extract_price_number_candidate(raw, &currency).unwrap();
                assert_eq!(actual, expected);
            }
            None => assert!(matches!(
                extract_price_number_candidate(raw, &currency),
                Err(PriceError::ParseFailure)
            )),
        }
    }

    // -----------------------------------------------------------------------
    // split_decimal
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("1234", None)]
    #[case("1234.56", Some(("1234", "56")))]
    #[case("1234,56", Some(("1234", "56")))]
    // dot is later → dot is decimal separator
    #[case("1.234,56", Some(("1.234", "56")))]
    // comma is later → comma is decimal separator
    #[case("1,234.56", Some(("1,234", "56")))]
    // lone comma/dot followed by exactly 3 digits → thousands separator, not decimal
    #[case("8,800", None)]
    #[case("1,234", None)]
    #[case("8.800", None)]
    #[case("1.234", None)]
    fn should_split_decimal_correctly_for_various_formats(
        #[case] input: &str,
        #[case] expected: Option<(&str, &str)>,
    ) {
        assert_eq!(split_decimal(input), expected);
    }

    // -----------------------------------------------------------------------
    // normalise_fraction
    // -----------------------------------------------------------------------

    #[rstest]
    #[case("56", 2, "56")]
    #[case("5", 2, "50")]
    #[case("", 2, "00")]
    #[case("123", 2, "12")]
    #[case("0", 2, "00")]
    fn should_normalise_fraction_to_correct_width(
        #[case] frac: &str,
        #[case] exponent: u32,
        #[case] expected: &str,
    ) {
        assert_eq!(normalise_fraction(frac, exponent), expected);
    }

    // -----------------------------------------------------------------------
    // is_price_on_request_marker
    // -----------------------------------------------------------------------

    #[rstest]
    // English
    #[case("Price on Request", true)]
    #[case("POA", true)]
    #[case("price available on request", true)]
    #[case("Please enquire", true)]
    #[case("Call for price", true)]
    // German
    #[case("Preis auf Anfrage", true)]
    #[case("auf Anfrage", true)]
    // French
    #[case("Prix sur demande", true)]
    #[case("sur demande", true)]
    // Italian
    #[case("Prezzo su richiesta", true)]
    #[case("Su Richiesta", true)]
    // Spanish
    #[case("Precio a consultar", true)]
    #[case("Consultar", true)]
    // Portuguese
    #[case("Preço sob consulta", true)]
    // Dutch
    #[case("Prijs op aanvraag", true)]
    // Polish
    #[case("Cena na zapytanie", true)]
    // Russian
    #[case("Цена по запросу", true)]
    // Chinese
    #[case("询价", true)]
    #[case("面议", true)]
    // Japanese
    #[case("価格はお問い合わせ", true)]
    // Arabic
    #[case("عند الطلب", true)]
    // Not markers
    #[case("$1200", false)]
    #[case("EUR 45", false)]
    #[case("1.500,00 €", false)]
    fn should_detect_price_on_request_markers(#[case] raw: &str, #[case] expected: bool) {
        assert_eq!(is_price_on_request_marker(raw), expected);
    }
}

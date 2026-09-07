use product_listing_core::listing_availability::ListingAvailability;
use regex::{Error as RegexError, RegexSet};
use std::sync::LazyLock;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

pub const MAX_AVAILABILITY_TEXT_BYTES: usize = 512;

const AVAILABLE_REGEX_PATTERNS: &[&str] = &[
    r"[1-9][0-9]*\s+available\b",
    r"\b(only\s+|just\s+)?[1-9][0-9]*\s+remaining\b",
    r"\b(only\s+)?[1-9][0-9]*\s+left\b",
    r"[1-9][0-9]*\s+in\s+stock\b",
    r"\bhurry\b.*[1-9][0-9]*",
    r"[1-9][0-9]*\s+vorrätig\b",
    r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+verfügbar\b",
    r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+auf\s+lager\b",
    r"(\bnur\s+)?(\bnoch\s+)?[1-9][0-9]*\s+stück\b",
    r"(\bplus\s+que\s+)?[1-9][0-9]*\s+en\s+stock\b",
    r"[1-9][0-9]*\s+disponibles?\b",
    r"\bil\s+(ne\s+)?reste\s+(que\s+)?[1-9][0-9]*\b",
    r"(\bsolo\s+)?[1-9][0-9]*\s+disponibles?\b",
    r"\bquedan\s+[1-9][0-9]*\b",
    r"(\bsolo\s+)?[1-9][0-9]*\s+disponibili\b",
    r"\brimangono\s+[1-9][0-9]*\b",
];

const OUT_OF_STOCK_REGEX_PATTERNS: &[&str] = &[
    r"\b0\s+available\b",
    r"\b0\s+remaining\b",
    r"\b0\s+left\b",
    r"\b0\s+in\s+stock\b",
    r"\b0\s+verfügbar\b",
    r"\b0\s+auf\s+lager\b",
    r"\b0\s+stück\b",
    r"\b0\s+en\s+stock\b",
    r"\b0\s+disponibles?\b",
    r"\b0\s+disponibili\b",
];

// Both results are immutable after their synchronized first construction.
static AVAILABLE_REGEX_SET: LazyLock<Result<RegexSet, RegexError>> =
    LazyLock::new(|| compile_regex_set(AVAILABLE_REGEX_PATTERNS));
static OUT_OF_STOCK_REGEX_SET: LazyLock<Result<RegexSet, RegexError>> =
    LazyLock::new(|| compile_regex_set(OUT_OF_STOCK_REGEX_PATTERNS));

// Test-only inspection avoids timing or allocator thresholds on the warm path.
#[cfg(test)]
static REGEX_SET_INITIALIZATIONS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingAvailabilityQuickCheck {
    Resolved(ListingAvailability),
    NoAssertion,
    Unsupported,
}

impl ListingAvailabilityQuickCheck {
    pub const fn availability(self) -> Option<ListingAvailability> {
        match self {
            Self::Resolved(value) => Some(value),
            Self::NoAssertion | Self::Unsupported => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AvailabilityNormalizationError {
    #[error("availability input exceeds the maximum length")]
    InputTooLong { len: usize, max: usize },
    #[error("availability input contains an embedded NUL")]
    EmbeddedNul,
    #[error("availability regex set configuration is invalid")]
    RegexSetCompilationFailed,
}

/// Recognizes generic availability evidence without provider mapping or I/O.
///
/// Empty state is an explicit absence. Unknown values stay unsupported so callers
/// cannot accidentally clear canonical availability.
pub fn quick_check_availability(
    raw: &str,
) -> Result<ListingAvailabilityQuickCheck, AvailabilityNormalizationError> {
    if raw.len() > MAX_AVAILABILITY_TEXT_BYTES {
        return Err(AvailabilityNormalizationError::InputTooLong {
            len: raw.len(),
            max: MAX_AVAILABILITY_TEXT_BYTES,
        });
    }
    if raw.contains('\0') {
        return Err(AvailabilityNormalizationError::EmbeddedNul);
    }
    quick_check_availability_with_regex_sets(
        raw,
        LazyLock::force(&AVAILABLE_REGEX_SET).as_ref(),
        LazyLock::force(&OUT_OF_STOCK_REGEX_SET).as_ref(),
    )
}

fn quick_check_availability_with_regex_sets(
    raw: &str,
    available_regex_set: Result<&RegexSet, &RegexError>,
    out_of_stock_regex_set: Result<&RegexSet, &RegexError>,
) -> Result<ListingAvailabilityQuickCheck, AvailabilityNormalizationError> {
    let available_regex_set = available_regex_set
        .map_err(|_| AvailabilityNormalizationError::RegexSetCompilationFailed)?;
    let out_of_stock_regex_set = out_of_stock_regex_set
        .map_err(|_| AvailabilityNormalizationError::RegexSetCompilationFailed)?;

    let value = raw.trim();
    if value.is_empty() {
        return Ok(ListingAvailabilityQuickCheck::NoAssertion);
    }

    if let Some(result) = schema_org_availability(value) {
        return Ok(result);
    }

    let normalized = value.to_lowercase();
    if let Some(result) = exact_availability(normalized.as_str()) {
        return Ok(result);
    }
    if let Some(result) = regex_availability(
        normalized.as_str(),
        available_regex_set,
        out_of_stock_regex_set,
    ) {
        return Ok(result);
    }

    Ok(ListingAvailabilityQuickCheck::Unsupported)
}

fn schema_org_availability(value: &str) -> Option<ListingAvailabilityQuickCheck> {
    let tail = value.rsplit('/').next().unwrap_or(value);
    let result = match tail {
        "InStock" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::InStock),
        "LimitedAvailability" => {
            ListingAvailabilityQuickCheck::Resolved(ListingAvailability::LimitedAvailability)
        }
        "BackOrder" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::BackOrder),
        "MadeToOrder" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::MadeToOrder),
        "PreOrder" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::PreOrder),
        "PreSale" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::PreSale),
        "Reserved" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::Reserved),
        "OutOfStock" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::OutOfStock),
        "SoldOut" => ListingAvailabilityQuickCheck::Resolved(ListingAvailability::SoldOut),
        "OnlineOnly" | "InStoreOnly" | "Discontinued" => ListingAvailabilityQuickCheck::NoAssertion,
        _ if value.contains("schema.org/") => ListingAvailabilityQuickCheck::Unsupported,
        _ => return None,
    };
    Some(result)
}

fn exact_availability(value: &str) -> Option<ListingAvailabilityQuickCheck> {
    let availability = match value {
        "available" | "add to cart" | "add to basket" | "buy now" | "in den warenkorb"
        | "verfügbar" | "auf lager" | "disponible" | "en stock" => ListingAvailability::Available,
        "in stock" => ListingAvailability::InStock,
        "reserved" | "on hold" | "reserviert" | "réservé" | "reserve" | "reservado"
        | "riservato" => ListingAvailability::Reserved,
        "sold" | "sold out" | "verkauft" | "ausverkauft" | "vendu" | "épuisé" | "vendido"
        | "venduto" => ListingAvailability::SoldOut,
        "out of stock" | "unavailable" => {
            return Some(ListingAvailabilityQuickCheck::Resolved(
                if value == "out of stock" {
                    ListingAvailability::OutOfStock
                } else {
                    ListingAvailability::Unavailable
                },
            ));
        }
        "listed" | "gelistet" | "listé" | "liste" | "listado" | "inserito" => {
            return Some(ListingAvailabilityQuickCheck::NoAssertion);
        }
        _ => return None,
    };
    Some(ListingAvailabilityQuickCheck::Resolved(availability))
}

fn regex_availability(
    value: &str,
    available_regex_set: &RegexSet,
    out_of_stock_regex_set: &RegexSet,
) -> Option<ListingAvailabilityQuickCheck> {
    if available_regex_set.is_match(value) {
        return Some(ListingAvailabilityQuickCheck::Resolved(
            ListingAvailability::Available,
        ));
    }
    if out_of_stock_regex_set.is_match(value) {
        return Some(ListingAvailabilityQuickCheck::Resolved(
            ListingAvailability::OutOfStock,
        ));
    }
    None
}

fn compile_regex_set(patterns: &[&str]) -> Result<RegexSet, RegexError> {
    let regex_set = RegexSet::new(patterns);

    #[cfg(test)]
    if regex_set.is_ok() {
        REGEX_SET_INITIALIZATIONS.fetch_add(1, Ordering::SeqCst);
    }

    regex_set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_resolve_exact_sold_value() {
        assert_eq!(
            quick_check_availability(" sold out "),
            Ok(ListingAvailabilityQuickCheck::Resolved(
                ListingAvailability::SoldOut
            ))
        );
    }

    #[test]
    fn should_resolve_regex_availability() {
        assert_eq!(
            quick_check_availability("Only 2 remaining"),
            Ok(ListingAvailabilityQuickCheck::Resolved(
                ListingAvailability::Available
            ))
        );
    }

    #[test]
    fn should_fail_closed_without_a_state_decision_when_regex_constants_are_invalid()
    -> Result<(), RegexError> {
        let available_regex_set = RegexSet::new(AVAILABLE_REGEX_PATTERNS)?;
        let invalid_out_of_stock_regex_set = compile_regex_set(&[r"["]);

        for value in ["sold out", "Only 2 remaining", "0 available"] {
            assert_eq!(
                quick_check_availability_with_regex_sets(
                    value,
                    Ok(&available_regex_set),
                    invalid_out_of_stock_regex_set.as_ref(),
                ),
                Err(AvailabilityNormalizationError::RegexSetCompilationFailed)
            );
        }

        Ok(())
    }

    #[test]
    fn should_fail_closed_for_blank_input_when_a_regex_set_is_invalid() -> Result<(), RegexError> {
        let available_regex_set = RegexSet::new(AVAILABLE_REGEX_PATTERNS)?;
        let out_of_stock_regex_set = RegexSet::new(OUT_OF_STOCK_REGEX_PATTERNS)?;
        let invalid_regex_set = compile_regex_set(&[r"["]);

        for value in ["", " \t\r\n"] {
            assert_eq!(
                quick_check_availability_with_regex_sets(
                    value,
                    invalid_regex_set.as_ref(),
                    Ok(&out_of_stock_regex_set),
                ),
                Err(AvailabilityNormalizationError::RegexSetCompilationFailed)
            );
            assert_eq!(
                quick_check_availability_with_regex_sets(
                    value,
                    Ok(&available_regex_set),
                    invalid_regex_set.as_ref(),
                ),
                Err(AvailabilityNormalizationError::RegexSetCompilationFailed)
            );
        }

        assert_eq!(
            quick_check_availability(" \t\r\n"),
            Ok(ListingAvailabilityQuickCheck::NoAssertion)
        );

        Ok(())
    }

    #[test]
    fn should_prefer_available_regex_evidence_when_both_outcomes_match() {
        assert_eq!(
            quick_check_availability("0 available; only 2 remaining"),
            Ok(ListingAvailabilityQuickCheck::Resolved(
                ListingAvailability::Available
            ))
        );
    }

    #[test]
    fn should_initialize_regex_sets_once_and_reuse_them_when_called_concurrently() {
        let start = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..64 {
                        assert_eq!(
                            quick_check_availability("Only 2 remaining"),
                            Ok(ListingAvailabilityQuickCheck::Resolved(
                                ListingAvailability::Available
                            ))
                        );
                        assert_eq!(
                            quick_check_availability("0 available"),
                            Ok(ListingAvailabilityQuickCheck::Resolved(
                                ListingAvailability::OutOfStock
                            ))
                        );
                    }
                });
            }
        });

        let initialization_count = REGEX_SET_INITIALIZATIONS.load(Ordering::SeqCst);
        assert_eq!(initialization_count, 2);
        assert!(std::ptr::eq(
            LazyLock::force(&AVAILABLE_REGEX_SET),
            LazyLock::force(&AVAILABLE_REGEX_SET)
        ));
        assert!(std::ptr::eq(
            LazyLock::force(&OUT_OF_STOCK_REGEX_SET),
            LazyLock::force(&OUT_OF_STOCK_REGEX_SET)
        ));

        assert_eq!(
            quick_check_availability_with_regex_sets(
                "Only 2 remaining",
                LazyLock::force(&AVAILABLE_REGEX_SET).as_ref(),
                LazyLock::force(&OUT_OF_STOCK_REGEX_SET).as_ref(),
            ),
            Ok(ListingAvailabilityQuickCheck::Resolved(
                ListingAvailability::Available
            ))
        );
        assert_eq!(
            quick_check_availability_with_regex_sets(
                "0 available",
                LazyLock::force(&AVAILABLE_REGEX_SET).as_ref(),
                LazyLock::force(&OUT_OF_STOCK_REGEX_SET).as_ref(),
            ),
            Ok(ListingAvailabilityQuickCheck::Resolved(
                ListingAvailability::OutOfStock
            ))
        );
        assert_eq!(
            REGEX_SET_INITIALIZATIONS.load(Ordering::SeqCst),
            initialization_count
        );
    }

    #[test]
    fn should_resolve_schema_org_availability() {
        assert_eq!(
            quick_check_availability("https://schema.org/OutOfStock"),
            Ok(ListingAvailabilityQuickCheck::Resolved(
                ListingAvailability::OutOfStock
            ))
        );
    }

    #[test]
    fn should_return_no_assertion_for_explicit_absence() {
        assert_eq!(
            quick_check_availability("listed"),
            Ok(ListingAvailabilityQuickCheck::NoAssertion)
        );
    }

    #[test]
    fn should_return_unsupported_for_unknown_plausible_value() {
        assert_eq!(
            quick_check_availability("limited availability soon"),
            Ok(ListingAvailabilityQuickCheck::Unsupported)
        );
    }

    #[test]
    fn should_reject_overlong_input() {
        let value = "x".repeat(MAX_AVAILABILITY_TEXT_BYTES + 1);
        assert!(matches!(
            quick_check_availability(value.as_str()),
            Err(AvailabilityNormalizationError::InputTooLong { .. })
        ));
    }

    #[test]
    fn should_reject_overlong_whitespace_input() {
        let value = " ".repeat(MAX_AVAILABILITY_TEXT_BYTES + 1);
        assert!(matches!(
            quick_check_availability(value.as_str()),
            Err(AvailabilityNormalizationError::InputTooLong { .. })
        ));
    }
}

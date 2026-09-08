use crate::FxRateId;
use money::{Currency, HasMinorUnitExponent, MonetaryAmount, Price};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use time::OffsetDateTime;

pub const FX_RATE_SCALE: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum FxRateSource {
    FxRatesApi,
}

impl FxRateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FxRatesApi => "fxratesapi",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FxRateGeneration(i64);

impl FxRateGeneration {
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for FxRateGeneration {
    type Error = FxRateSnapshotError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value <= 0 {
            return Err(FxRateSnapshotError::InvalidGeneration);
        }
        Ok(Self(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxRateQuote {
    currency: Currency,
    units_per_eur: u64,
}

impl FxRateQuote {
    pub fn new(currency: Currency, units_per_eur: u64) -> Self {
        Self {
            currency,
            units_per_eur,
        }
    }

    pub fn currency(self) -> Currency {
        self.currency
    }

    pub fn units_per_eur(self) -> u64 {
        self.units_per_eur
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFxRateSnapshot {
    id: FxRateId,
    captured_at: OffsetDateTime,
    source: FxRateSource,
    quotes: Vec<FxRateQuote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxRateSnapshot {
    id: FxRateId,
    generation: FxRateGeneration,
    captured_at: OffsetDateTime,
    source: FxRateSource,
    quotes: Vec<FxRateQuote>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingMode {
    Floor,
    HalfUp,
    Ceil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayAmountRange {
    pub lower: Option<MonetaryAmount>,
    pub upper: Option<MonetaryAmount>,
}

impl DisplayAmountRange {
    pub fn contains(self, amount: MonetaryAmount) -> bool {
        self.lower.is_none_or(|lower| amount >= lower)
            && self.upper.is_none_or(|upper| amount <= upper)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceAmountRange {
    pub lower: MonetaryAmount,
    pub upper: Option<MonetaryAmount>,
}

impl SourceAmountRange {
    pub fn contains(self, amount: MonetaryAmount) -> bool {
        amount >= self.lower && self.upper.is_none_or(|upper| amount <= upper)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FxRateSnapshotError {
    #[error("FX rate base currency must be EUR")]
    NonEurBaseCurrency,
    #[error("FX rate quote is duplicated for {0}")]
    DuplicateQuote(Currency),
    #[error("FX rate quote is missing for {0}")]
    MissingQuote(Currency),
    #[error("EUR units per EUR must equal the FX rate scale")]
    InvalidEurQuote,
    #[error("FX rate quote is zero for {0}")]
    ZeroQuote(Currency),
    #[error("FX rate quote cannot be stored in PostgreSQL bigint for {0}")]
    QuoteCannotBePersisted(Currency),
    #[error("FX rate snapshot generation must be positive")]
    InvalidGeneration,
    #[error("FX rate source is invalid")]
    InvalidSource,
    #[error("FX conversion overflowed")]
    ConversionOverflow,
}

impl NewFxRateSnapshot {
    pub fn capture_eur(
        id: FxRateId,
        captured_at: OffsetDateTime,
        source: FxRateSource,
        base: Currency,
        quotes: impl IntoIterator<Item = FxRateQuote>,
    ) -> Result<Self, FxRateSnapshotError> {
        if base != Currency::Eur {
            return Err(FxRateSnapshotError::NonEurBaseCurrency);
        }
        let quotes = validate_quotes(quotes)?;
        Ok(Self {
            id,
            captured_at,
            source,
            quotes,
        })
    }

    pub fn id(&self) -> FxRateId {
        self.id
    }

    pub fn captured_at(&self) -> OffsetDateTime {
        self.captured_at
    }

    pub fn source(&self) -> FxRateSource {
        self.source
    }

    pub fn quotes(&self) -> &[FxRateQuote] {
        &self.quotes
    }

    pub fn into_persisted(self, generation: FxRateGeneration) -> FxRateSnapshot {
        FxRateSnapshot {
            id: self.id,
            generation,
            captured_at: self.captured_at,
            source: self.source,
            quotes: self.quotes,
        }
    }
}

impl FxRateSnapshot {
    pub fn rehydrate(
        id: FxRateId,
        generation: FxRateGeneration,
        captured_at: OffsetDateTime,
        source: FxRateSource,
        quotes: impl IntoIterator<Item = FxRateQuote>,
    ) -> Result<Self, FxRateSnapshotError> {
        Ok(Self {
            id,
            generation,
            captured_at,
            source,
            quotes: validate_quotes(quotes)?,
        })
    }

    pub fn id(&self) -> FxRateId {
        self.id
    }

    pub fn generation(&self) -> FxRateGeneration {
        self.generation
    }

    pub fn captured_at(&self) -> OffsetDateTime {
        self.captured_at
    }

    pub fn source(&self) -> FxRateSource {
        self.source
    }

    pub fn quotes(&self) -> &[FxRateQuote] {
        &self.quotes
    }

    pub fn convert(
        &self,
        source: Price,
        target_currency: Currency,
        rounding: RoundingMode,
    ) -> Result<Price, FxRateSnapshotError> {
        if source.currency == target_currency {
            return Ok(source);
        }
        let (numerator_multiplier, denominator) =
            self.conversion_ratio(source.currency, target_currency)?;
        let numerator = u128::from(u64::from(source.monetary_amount))
            .checked_mul(numerator_multiplier)
            .ok_or(FxRateSnapshotError::ConversionOverflow)?;
        let amount = round_division(numerator, denominator, rounding)?;
        Ok(Price::new(MonetaryAmount::from(amount), target_currency))
    }

    /// Returns the exact inclusive source interval whose half-up display conversion
    /// lies inside `display_range`. `None` means no source amount can match.
    pub fn compile_source_range(
        &self,
        source_currency: Currency,
        target_currency: Currency,
        display_range: DisplayAmountRange,
    ) -> Result<Option<SourceAmountRange>, FxRateSnapshotError> {
        if let (Some(lower), Some(upper)) = (display_range.lower, display_range.upper)
            && lower > upper
        {
            return Ok(None);
        }
        if source_currency == target_currency {
            return Ok(Some(SourceAmountRange {
                lower: display_range.lower.unwrap_or(MonetaryAmount::from(0_u64)),
                upper: display_range.upper,
            }));
        }

        let (multiplier, divisor) = self.conversion_ratio(source_currency, target_currency)?;
        let half = divisor / 2;
        let lower = match display_range.lower {
            Some(lower) => {
                let threshold = u128::from(u64::from(lower))
                    .checked_mul(divisor)
                    .ok_or(FxRateSnapshotError::ConversionOverflow)?;
                let required = threshold.saturating_sub(half);
                let value = ceil_division(required, multiplier)?;
                match u64::try_from(value) {
                    Ok(value) => MonetaryAmount::from(value),
                    Err(_) => return Ok(None),
                }
            }
            None => MonetaryAmount::from(0_u64),
        };
        let upper = match display_range.upper {
            Some(upper) => {
                let exclusive = u128::from(u64::from(upper))
                    .checked_add(1)
                    .ok_or(FxRateSnapshotError::ConversionOverflow)?
                    .checked_mul(divisor)
                    .ok_or(FxRateSnapshotError::ConversionOverflow)?;
                let maximum = exclusive
                    .checked_sub(half)
                    .and_then(|value| value.checked_sub(1))
                    .ok_or(FxRateSnapshotError::ConversionOverflow)?;
                match u64::try_from(maximum / multiplier) {
                    Ok(value) => Some(MonetaryAmount::from(value)),
                    Err(_) => None,
                }
            }
            None => None,
        };
        if upper.is_some_and(|upper| lower > upper) {
            return Ok(None);
        }
        Ok(Some(SourceAmountRange { lower, upper }))
    }

    fn conversion_ratio(
        &self,
        source_currency: Currency,
        target_currency: Currency,
    ) -> Result<(u128, u128), FxRateSnapshotError> {
        let target_units_per_eur = u128::from(self.units_per_eur(target_currency)?);
        let source_units_per_eur = u128::from(self.units_per_eur(source_currency)?);
        let target_factor = power_of_ten(target_currency.minor_unit_exponent().0)?;
        let source_factor = power_of_ten(source_currency.minor_unit_exponent().0)?;
        let numerator_multiplier = target_units_per_eur
            .checked_mul(target_factor)
            .ok_or(FxRateSnapshotError::ConversionOverflow)?;
        let denominator = source_units_per_eur
            .checked_mul(source_factor)
            .ok_or(FxRateSnapshotError::ConversionOverflow)?;
        Ok((numerator_multiplier, denominator))
    }

    fn units_per_eur(&self, currency: Currency) -> Result<u64, FxRateSnapshotError> {
        self.quotes
            .iter()
            .find(|quote| quote.currency == currency)
            .map(|quote| quote.units_per_eur)
            .ok_or(FxRateSnapshotError::MissingQuote(currency))
    }
}

fn validate_quotes(
    quotes: impl IntoIterator<Item = FxRateQuote>,
) -> Result<Vec<FxRateQuote>, FxRateSnapshotError> {
    let mut by_currency = HashMap::new();
    for quote in quotes {
        if quote.units_per_eur == 0 {
            return Err(FxRateSnapshotError::ZeroQuote(quote.currency));
        }
        if quote.units_per_eur > i64::MAX as u64 {
            return Err(FxRateSnapshotError::QuoteCannotBePersisted(quote.currency));
        }
        if by_currency
            .insert(quote.currency, quote.units_per_eur)
            .is_some()
        {
            return Err(FxRateSnapshotError::DuplicateQuote(quote.currency));
        }
    }

    if by_currency.get(&Currency::Eur) != Some(&FX_RATE_SCALE) {
        return Err(FxRateSnapshotError::InvalidEurQuote);
    }

    Currency::iter()
        .map(|currency| {
            by_currency
                .remove(&currency)
                .map(|units_per_eur| FxRateQuote::new(currency, units_per_eur))
                .ok_or(FxRateSnapshotError::MissingQuote(currency))
        })
        .collect()
}

fn power_of_ten(exponent: u8) -> Result<u128, FxRateSnapshotError> {
    10_u128
        .checked_pow(u32::from(exponent))
        .ok_or(FxRateSnapshotError::ConversionOverflow)
}

fn round_division(
    numerator: u128,
    denominator: u128,
    mode: RoundingMode,
) -> Result<u64, FxRateSnapshotError> {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let increment = match mode {
        RoundingMode::Floor => 0,
        RoundingMode::HalfUp => {
            let rounds_up = remainder > denominator / 2
                || (denominator.is_multiple_of(2) && remainder == denominator / 2);
            u128::from(u8::from(rounds_up))
        }
        RoundingMode::Ceil => u128::from(u8::from(remainder != 0)),
    };
    let rounded = quotient
        .checked_add(increment)
        .ok_or(FxRateSnapshotError::ConversionOverflow)?;
    u64::try_from(rounded).map_err(|_| FxRateSnapshotError::ConversionOverflow)
}

fn ceil_division(value: u128, divisor: u128) -> Result<u128, FxRateSnapshotError> {
    Ok(value.div_ceil(divisor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> FxRateSnapshot {
        let quotes = Currency::iter().map(|currency| {
            let units_per_eur = match currency {
                Currency::Eur => FX_RATE_SCALE,
                Currency::Jpy => 160_000_000,
                Currency::Usd => 1_100_000,
                Currency::Gbp => 850_000,
                _ => 1_250_000,
            };
            FxRateQuote::new(currency, units_per_eur)
        });
        let new = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            quotes,
        );
        match new {
            Ok(snapshot) => snapshot.into_persisted(
                FxRateGeneration::try_from(1)
                    .unwrap_or_else(|error| panic!("valid generation: {error}")),
            ),
            Err(error) => panic!("valid snapshot: {error}"),
        }
    }

    fn price(amount: u64, currency: Currency) -> Price {
        Price::new(MonetaryAmount::from(amount), currency)
    }

    #[test]
    fn should_define_unique_canonical_source_identifiers() {
        use std::collections::HashSet;

        let sources = FxRateSource::iter().collect::<Vec<_>>();
        let identifiers = sources
            .iter()
            .map(|source| source.as_str())
            .collect::<HashSet<_>>();

        assert_eq!(sources.len(), identifiers.len());
        assert_eq!(
            vec!["fxratesapi"],
            sources
                .into_iter()
                .map(FxRateSource::as_str)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn should_reject_incomplete_duplicate_zero_bad_eur_and_unpersistable_quotes() {
        let complete = || {
            Currency::iter()
                .map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE))
                .collect::<Vec<_>>()
        };
        let missing = complete()
            .into_iter()
            .filter(|quote| quote.currency() != Currency::Gbp);
        assert!(matches!(
            NewFxRateSnapshot::capture_eur(
                FxRateId::new(),
                OffsetDateTime::UNIX_EPOCH,
                FxRateSource::FxRatesApi,
                Currency::Eur,
                missing
            ),
            Err(FxRateSnapshotError::MissingQuote(Currency::Gbp))
        ));

        let mut duplicate = complete();
        duplicate.push(FxRateQuote::new(Currency::Usd, FX_RATE_SCALE));
        assert!(matches!(
            NewFxRateSnapshot::capture_eur(
                FxRateId::new(),
                OffsetDateTime::UNIX_EPOCH,
                FxRateSource::FxRatesApi,
                Currency::Eur,
                duplicate
            ),
            Err(FxRateSnapshotError::DuplicateQuote(Currency::Usd))
        ));

        let zero = Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Usd {
                    0
                } else {
                    FX_RATE_SCALE
                },
            )
        });
        assert!(matches!(
            NewFxRateSnapshot::capture_eur(
                FxRateId::new(),
                OffsetDateTime::UNIX_EPOCH,
                FxRateSource::FxRatesApi,
                Currency::Eur,
                zero
            ),
            Err(FxRateSnapshotError::ZeroQuote(Currency::Usd))
        ));

        let bad_eur = Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Eur {
                    FX_RATE_SCALE - 1
                } else {
                    FX_RATE_SCALE
                },
            )
        });
        assert_eq!(
            Err(FxRateSnapshotError::InvalidEurQuote),
            NewFxRateSnapshot::capture_eur(
                FxRateId::new(),
                OffsetDateTime::UNIX_EPOCH,
                FxRateSource::FxRatesApi,
                Currency::Eur,
                bad_eur
            )
        );

        let too_large = Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Usd {
                    i64::MAX as u64 + 1
                } else {
                    FX_RATE_SCALE
                },
            )
        });
        assert!(matches!(
            NewFxRateSnapshot::capture_eur(
                FxRateId::new(),
                OffsetDateTime::UNIX_EPOCH,
                FxRateSource::FxRatesApi,
                Currency::Eur,
                too_large
            ),
            Err(FxRateSnapshotError::QuoteCannotBePersisted(Currency::Usd))
        ));
    }

    #[test]
    fn should_convert_across_eur_usd_and_jpy_with_all_rounding_modes() {
        let snapshot = snapshot();
        assert_eq!(
            Ok(price(500, Currency::Eur)),
            snapshot.convert(
                price(500, Currency::Eur),
                Currency::Eur,
                RoundingMode::HalfUp
            )
        );
        assert_eq!(
            Ok(price(110, Currency::Usd)),
            snapshot.convert(
                price(100, Currency::Eur),
                Currency::Usd,
                RoundingMode::HalfUp
            )
        );
        assert_eq!(
            Ok(price(91, Currency::Eur)),
            snapshot.convert(
                price(100, Currency::Usd),
                Currency::Eur,
                RoundingMode::HalfUp
            )
        );
        assert_eq!(
            Ok(price(160, Currency::Jpy)),
            snapshot.convert(
                price(100, Currency::Eur),
                Currency::Jpy,
                RoundingMode::HalfUp
            )
        );
        assert_eq!(
            Ok(price(63, Currency::Eur)),
            snapshot.convert(
                price(100, Currency::Jpy),
                Currency::Eur,
                RoundingMode::HalfUp
            )
        );
        assert_eq!(
            Ok(price(69, Currency::Usd)),
            snapshot.convert(
                price(100, Currency::Jpy),
                Currency::Usd,
                RoundingMode::HalfUp
            )
        );
        assert_eq!(
            Ok(price(145, Currency::Jpy)),
            snapshot.convert(
                price(100, Currency::Usd),
                Currency::Jpy,
                RoundingMode::HalfUp
            )
        );

        let quotes = [
            FxRateQuote::new(Currency::Eur, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Usd, 1_000_000),
            FxRateQuote::new(Currency::Gbp, 1_500_000),
            FxRateQuote::new(Currency::Aud, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Cad, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Nzd, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Cny, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Brl, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Pln, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Try, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Jpy, 1_000_000),
            FxRateQuote::new(Currency::Czk, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Rub, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Aed, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Sar, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Hkd, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Sgd, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Chf, FX_RATE_SCALE),
            FxRateQuote::new(Currency::Zar, FX_RATE_SCALE),
        ];
        let precise = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            quotes,
        )
        .unwrap_or_else(|error| panic!("valid snapshot: {error}"))
        .into_persisted(
            FxRateGeneration::try_from(1)
                .unwrap_or_else(|error| panic!("valid generation: {error}")),
        );
        assert_eq!(
            Ok(price(1, Currency::Gbp)),
            precise.convert(price(1, Currency::Usd), Currency::Gbp, RoundingMode::Floor)
        );
        assert_eq!(
            Ok(price(2, Currency::Gbp)),
            precise.convert(price(1, Currency::Usd), Currency::Gbp, RoundingMode::HalfUp)
        );
        assert_eq!(
            Ok(price(2, Currency::Gbp)),
            precise.convert(price(1, Currency::Usd), Currency::Gbp, RoundingMode::Ceil)
        );
    }

    #[test]
    fn should_return_typed_error_when_conversion_intermediate_overflows() {
        let snapshot = snapshot();
        assert_eq!(
            Err(FxRateSnapshotError::ConversionOverflow),
            snapshot.convert(
                price(u64::MAX, Currency::Eur),
                Currency::Jpy,
                RoundingMode::HalfUp
            )
        );
    }

    #[test]
    fn should_keep_percolation_and_inverse_range_membership_identical() {
        let snapshot = snapshot();
        let ranges = [
            DisplayAmountRange {
                lower: None,
                upper: Some(MonetaryAmount::from(5_u64)),
            },
            DisplayAmountRange {
                lower: Some(MonetaryAmount::from(3_u64)),
                upper: None,
            },
            DisplayAmountRange {
                lower: Some(MonetaryAmount::from(5_u64)),
                upper: Some(MonetaryAmount::from(5_u64)),
            },
            DisplayAmountRange {
                lower: Some(MonetaryAmount::from(0_u64)),
                upper: Some(MonetaryAmount::from(1_u64)),
            },
            DisplayAmountRange {
                lower: Some(MonetaryAmount::from(1_000_000_u64)),
                upper: Some(MonetaryAmount::from(1_000_100_u64)),
            },
        ];
        let amounts = [
            0_u64, 1, 2, 3, 4, 5, 49, 50, 51, 99, 100, 101, 10_001, 1_000_000,
        ];

        for source_currency in Currency::iter() {
            for target_currency in Currency::iter() {
                for range in ranges {
                    let compiled = snapshot
                        .compile_source_range(source_currency, target_currency, range)
                        .unwrap_or_else(|error| panic!("range must compile: {error}"));
                    for amount in amounts {
                        let displayed = snapshot
                            .convert(
                                price(amount, source_currency),
                                target_currency,
                                RoundingMode::HalfUp,
                            )
                            .unwrap_or_else(|error| panic!("conversion must succeed: {error}"));
                        assert_eq!(
                            range.contains(displayed.monetary_amount),
                            compiled.is_some_and(
                                |compiled| compiled.contains(MonetaryAmount::from(amount))
                            ),
                            "{source_currency:?} -> {target_currency:?}, amount {amount}, range {range:?}",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn should_compile_exact_inverse_ranges_for_every_supported_currency_pair() {
        let snapshot = snapshot();
        for source_currency in Currency::iter() {
            for target_currency in Currency::iter() {
                for display_range in [
                    DisplayAmountRange {
                        lower: None,
                        upper: Some(MonetaryAmount::from(5_u64)),
                    },
                    DisplayAmountRange {
                        lower: Some(MonetaryAmount::from(3_u64)),
                        upper: None,
                    },
                    DisplayAmountRange {
                        lower: Some(MonetaryAmount::from(3_u64)),
                        upper: Some(MonetaryAmount::from(12_u64)),
                    },
                ] {
                    let compiled = snapshot.compile_source_range(
                        source_currency,
                        target_currency,
                        display_range,
                    );
                    let compiled = match compiled {
                        Ok(Some(compiled)) => compiled,
                        Ok(None) => continue,
                        Err(error) => panic!("range must compile: {error}"),
                    };
                    for amount in 0..=500 {
                        let source = price(amount, source_currency);
                        let displayed =
                            snapshot.convert(source, target_currency, RoundingMode::HalfUp);
                        let displayed = match displayed {
                            Ok(value) => value,
                            Err(error) => panic!("small conversion must succeed: {error}"),
                        };
                        assert_eq!(
                            display_range.contains(displayed.monetary_amount),
                            compiled.contains(source.monetary_amount),
                            "{source_currency:?} -> {target_currency:?}, amount {amount}"
                        );
                    }
                }
            }
        }
    }
}

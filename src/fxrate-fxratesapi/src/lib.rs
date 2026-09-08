use application::error::{box_error, static_error};
use money::Currency;

use fxrate_service::ports::{
    FxRateQuote, FxRateQuoteProvider, FxRateQuoteProviderError, FxRateQuoteSet,
};
use serde::Deserialize;
use std::collections::HashMap;
use strum::IntoEnumIterator;

const FX_RATES_API_URL: &str = "https://api.fxratesapi.com/latest";
const FX_RATE_DECIMAL_PLACES: i64 = 6;

#[derive(Debug, Clone)]
pub struct FxRatesApiQuoteProvider {
    client: reqwest::Client,
    token: String,
}

#[derive(Debug, Deserialize)]
struct FxRatesApiResponse {
    success: bool,
    base: ProviderCurrency,
    rates: HashMap<ProviderCurrency, serde_json::Number>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ProviderCurrency {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
    Cny,
    Brl,
    Pln,
    Try,
    Jpy,
    Czk,
    Rub,
    Aed,
    Sar,
    Hkd,
    Sgd,
    Chf,
    Zar,
}

impl From<ProviderCurrency> for Currency {
    fn from(currency: ProviderCurrency) -> Self {
        match currency {
            ProviderCurrency::Eur => Self::Eur,
            ProviderCurrency::Gbp => Self::Gbp,
            ProviderCurrency::Usd => Self::Usd,
            ProviderCurrency::Aud => Self::Aud,
            ProviderCurrency::Cad => Self::Cad,
            ProviderCurrency::Nzd => Self::Nzd,
            ProviderCurrency::Cny => Self::Cny,
            ProviderCurrency::Brl => Self::Brl,
            ProviderCurrency::Pln => Self::Pln,
            ProviderCurrency::Try => Self::Try,
            ProviderCurrency::Jpy => Self::Jpy,
            ProviderCurrency::Czk => Self::Czk,
            ProviderCurrency::Rub => Self::Rub,
            ProviderCurrency::Aed => Self::Aed,
            ProviderCurrency::Sar => Self::Sar,
            ProviderCurrency::Hkd => Self::Hkd,
            ProviderCurrency::Sgd => Self::Sgd,
            ProviderCurrency::Chf => Self::Chf,
            ProviderCurrency::Zar => Self::Zar,
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("FX rate must be a positive decimal number within the supported range")]
struct InvalidFxRate;

impl FxRatesApiQuoteProvider {
    pub fn new(client: reqwest::Client, token: impl Into<String>) -> Self {
        Self {
            client,
            token: token.into(),
        }
    }
}

#[async_trait::async_trait]
impl FxRateQuoteProvider for FxRatesApiQuoteProvider {
    async fn fetch_eur_quotes(&self) -> Result<FxRateQuoteSet, FxRateQuoteProviderError> {
        let currencies = Currency::iter()
            .filter(|currency| *currency != Currency::Eur)
            .map(|currency| currency.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let response = self
            .client
            .get(FX_RATES_API_URL)
            .query(&[("base", "EUR"), ("currencies", currencies.as_str())])
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|source| FxRateQuoteProviderError::RequestFailed {
                source: box_error(source),
            })?
            .error_for_status()
            .map_err(|source| FxRateQuoteProviderError::RequestFailed {
                source: box_error(source),
            })?
            .json::<FxRatesApiResponse>()
            .await
            .map_err(|source| FxRateQuoteProviderError::InvalidResponse {
                source: box_error(source),
            })?;
        if !response.success {
            return Err(FxRateQuoteProviderError::InvalidResponse {
                source: static_error("FxRatesApi returned success=false"),
            });
        }

        let quotes = response
            .rates
            .into_iter()
            .map(|(currency, rate)| {
                decimal_rate_to_scaled_integer(&rate).map(|units_per_eur| FxRateQuote {
                    currency: currency.into(),
                    units_per_eur,
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| FxRateQuoteProviderError::InvalidResponse {
                source: box_error(source),
            })?;

        Ok(FxRateQuoteSet {
            base: response.base.into(),
            quotes,
        })
    }
}

fn decimal_rate_to_scaled_integer(value: &serde_json::Number) -> Result<u64, InvalidFxRate> {
    let value = value.to_string();
    let (significand, exponent) = match value.split_once(['e', 'E']) {
        Some((significand, exponent)) => (
            significand,
            exponent.parse::<i32>().map_err(|_| InvalidFxRate)?,
        ),
        None => (value.as_str(), 0),
    };
    let (whole, fractional) = match significand.split_once('.') {
        Some((whole, fractional)) => (whole, fractional),
        None => (significand, ""),
    };
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(InvalidFxRate);
    }

    let significant = format!("{whole}{fractional}")
        .parse::<u128>()
        .map_err(|_| InvalidFxRate)?;
    let shift = i64::from(exponent) - i64::try_from(fractional.len()).map_err(|_| InvalidFxRate)?
        + FX_RATE_DECIMAL_PLACES;
    let scaled = if shift >= 0 {
        significant
            .checked_mul(power_of_ten(
                u32::try_from(shift).map_err(|_| InvalidFxRate)?,
            )?)
            .ok_or(InvalidFxRate)?
    } else {
        let divisor = power_of_ten(u32::try_from(-shift).map_err(|_| InvalidFxRate)?)?;
        let quotient = significant / divisor;
        let remainder = significant % divisor;
        let rounds_up =
            remainder > divisor / 2 || (divisor.is_multiple_of(2) && remainder == divisor / 2);
        quotient
            .checked_add(u128::from(u8::from(rounds_up)))
            .ok_or(InvalidFxRate)?
    };
    let scaled = u64::try_from(scaled).map_err(|_| InvalidFxRate)?;
    if scaled == 0 || scaled > i64::MAX as u64 {
        return Err(InvalidFxRate);
    }
    Ok(scaled)
}

fn power_of_ten(exponent: u32) -> Result<u128, InvalidFxRate> {
    10_u128.checked_pow(exponent).ok_or(InvalidFxRate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fxrate_core::FX_RATE_SCALE;

    fn number(value: &str) -> serde_json::Number {
        serde_json::from_str(value)
            .unwrap_or_else(|error| panic!("test number must be valid JSON: {error}"))
    }

    #[test]
    fn should_parse_rates_to_scaled_integers_with_half_up_rounding() {
        for (input, expected) in [
            ("1", FX_RATE_SCALE),
            ("1.25", 1_250_000),
            ("0.0000004", 0),
            ("0.0000005", 1),
            ("1.0000004", 1_000_000),
            ("1.0000005", 1_000_001),
            ("1.000000500000", 1_000_001),
            ("1.25e0", 1_250_000),
            ("125e-2", 1_250_000),
        ] {
            if expected == 0 {
                assert!(decimal_rate_to_scaled_integer(&number(input)).is_err());
            } else {
                assert_eq!(Ok(expected), decimal_rate_to_scaled_integer(&number(input)));
            }
        }
    }

    #[test]
    fn should_reject_zero_negative_and_unpersistable_rates() {
        for input in ["0", "-1", "9223372036855"] {
            assert!(decimal_rate_to_scaled_integer(&number(input)).is_err());
        }
    }
}

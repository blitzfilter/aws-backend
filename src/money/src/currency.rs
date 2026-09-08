use crate::{MonetaryAmount, Price};
use std::collections::HashMap;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct MinorUnitExponent(pub u8);

impl From<u8> for MinorUnitExponent {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

impl From<MinorUnitExponent> for u8 {
    fn from(value: MinorUnitExponent) -> Self {
        value.0
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Debug,
    Default,
    Hash,
    strum_macros::EnumIter,
    strum_macros::Display,
    strum_macros::EnumCount,
)]
pub enum Currency {
    #[default]
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

impl Currency {
    pub fn resolve(
        preferred: &[Currency],
        available: HashMap<Currency, MonetaryAmount>,
    ) -> Option<Price> {
        let mut available = available;
        preferred
            .iter()
            .find_map(|currency| {
                available
                    .remove(currency)
                    .map(|amount| Price::new(amount, *currency))
            })
            .or_else(|| {
                available
                    .remove(&Currency::Eur)
                    .map(|amount| Price::new(amount, Currency::Eur))
            })
            .or_else(|| {
                available
                    .remove(&Currency::Usd)
                    .map(|amount| Price::new(amount, Currency::Usd))
            })
            .or_else(|| {
                available
                    .remove(&Currency::Gbp)
                    .map(|amount| Price::new(amount, Currency::Gbp))
            })
            .or_else(|| {
                available
                    .into_iter()
                    .next()
                    .map(|(currency, amount)| Price::new(amount, currency))
            })
    }

    pub fn extract_amount(
        self,
        native: &Option<Price>,
        other: &HashMap<Currency, MonetaryAmount>,
    ) -> Option<MonetaryAmount> {
        other.get(&self).copied().or_else(|| {
            native
                .filter(|price| price.currency == self)
                .map(|price| price.monetary_amount)
        })
    }

    pub fn currency_symbol(self) -> &'static str {
        match self {
            Currency::Eur => "€",
            Currency::Gbp => "£",
            Currency::Usd => "$",
            Currency::Aud => "A$",
            Currency::Cad => "C$",
            Currency::Nzd => "NZ$",
            Currency::Cny => "CN¥",
            Currency::Brl => "R$",
            Currency::Pln => "zł",
            Currency::Try => "₺",
            Currency::Jpy => "¥",
            Currency::Czk => "Kč",
            Currency::Rub => "₽",
            Currency::Aed => "د.إ",
            Currency::Sar => "﷼",
            Currency::Hkd => "HK$",
            Currency::Sgd => "S$",
            Currency::Chf => "CHF",
            Currency::Zar => "R",
        }
    }

    pub fn decimal_separator(self) -> &'static str {
        match self {
            Currency::Eur
            | Currency::Brl
            | Currency::Pln
            | Currency::Try
            | Currency::Czk
            | Currency::Rub => ",",
            Currency::Gbp
            | Currency::Usd
            | Currency::Aud
            | Currency::Cad
            | Currency::Nzd
            | Currency::Cny
            | Currency::Jpy
            | Currency::Aed
            | Currency::Sar
            | Currency::Hkd
            | Currency::Sgd
            | Currency::Chf
            | Currency::Zar => ".",
        }
    }

    pub fn is_leading_sign(self) -> bool {
        !matches!(
            self,
            Currency::Eur
                | Currency::Pln
                | Currency::Try
                | Currency::Czk
                | Currency::Rub
                | Currency::Aed
                | Currency::Sar
        )
    }

    pub fn from_code(value: &str) -> Option<Self> {
        use strum::IntoEnumIterator;

        Self::iter().find(|currency| currency.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Currency::Eur => "EUR",
            Currency::Gbp => "GBP",
            Currency::Usd => "USD",
            Currency::Aud => "AUD",
            Currency::Cad => "CAD",
            Currency::Nzd => "NZD",
            Currency::Cny => "CNY",
            Currency::Brl => "BRL",
            Currency::Pln => "PLN",
            Currency::Try => "TRY",
            Currency::Jpy => "JPY",
            Currency::Czk => "CZK",
            Currency::Rub => "RUB",
            Currency::Aed => "AED",
            Currency::Sar => "SAR",
            Currency::Hkd => "HKD",
            Currency::Sgd => "SGD",
            Currency::Chf => "CHF",
            Currency::Zar => "ZAR",
        }
    }
}

pub trait HasMinorUnitExponent {
    fn minor_unit_exponent(&self) -> MinorUnitExponent;
}

impl HasMinorUnitExponent for Currency {
    fn minor_unit_exponent(&self) -> MinorUnitExponent {
        match self {
            Currency::Jpy => MinorUnitExponent(0),
            _ => MinorUnitExponent(2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn should_round_trip_all_canonical_currency_codes() {
        for currency in Currency::iter() {
            assert_eq!(Some(currency), Currency::from_code(currency.as_str()));
        }
        assert_eq!(None, Currency::from_code("eur"));
    }
}

use crate::currency::command_data::CurrencyCommandData;
use crate::currency::data::CurrencyData;
use crate::currency::record::CurrencyRecord;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct MinorUnitExponent(pub u8);

impl From<u8> for MinorUnitExponent {
    fn from(item: u8) -> Self {
        Self(item)
    }
}

impl From<MinorUnitExponent> for u8 {
    fn from(item: MinorUnitExponent) -> Self {
        item.0
    }
}

#[derive(
    Copy,
    Clone,
    Eq,
    PartialEq,
    Debug,
    Hash,
    strum_macros::EnumIter,
    strum_macros::Display,
    strum_macros::EnumCount,
)]
pub enum Currency {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
}

pub trait HasMinorUnitExponent {
    fn minor_unit_exponent(&self) -> MinorUnitExponent;
}

impl HasMinorUnitExponent for Currency {
    fn minor_unit_exponent(&self) -> MinorUnitExponent {
        match self {
            Currency::Eur => MinorUnitExponent(2),
            Currency::Gbp => MinorUnitExponent(2),
            Currency::Usd => MinorUnitExponent(2),
            Currency::Aud => MinorUnitExponent(2),
            Currency::Cad => MinorUnitExponent(2),
            Currency::Nzd => MinorUnitExponent(2),
        }
    }
}

impl Default for Currency {
    fn default() -> Self {
        Self::Eur
    }
}

impl From<CurrencyCommandData> for Currency {
    fn from(cmd: CurrencyCommandData) -> Self {
        match cmd {
            CurrencyCommandData::Eur => Currency::Eur,
            CurrencyCommandData::Gbp => Currency::Gbp,
            CurrencyCommandData::Usd => Currency::Usd,
            CurrencyCommandData::Aud => Currency::Aud,
            CurrencyCommandData::Cad => Currency::Cad,
            CurrencyCommandData::Nzd => Currency::Nzd,
        }
    }
}

impl From<CurrencyRecord> for Currency {
    fn from(cmd: CurrencyRecord) -> Self {
        match cmd {
            CurrencyRecord::Eur => Currency::Eur,
            CurrencyRecord::Gbp => Currency::Gbp,
            CurrencyRecord::Usd => Currency::Usd,
            CurrencyRecord::Aud => Currency::Aud,
            CurrencyRecord::Cad => Currency::Cad,
            CurrencyRecord::Nzd => Currency::Nzd,
        }
    }
}

impl From<CurrencyData> for Currency {
    fn from(data: CurrencyData) -> Self {
        match data {
            CurrencyData::Eur => Currency::Eur,
            CurrencyData::Gbp => Currency::Gbp,
            CurrencyData::Usd => Currency::Usd,
            CurrencyData::Aud => Currency::Aud,
            CurrencyData::Cad => Currency::Cad,
            CurrencyData::Nzd => Currency::Nzd,
        }
    }
}

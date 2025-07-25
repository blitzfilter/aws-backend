use crate::currency::command_data::CurrencyCommandData;
use crate::currency::data::CurrencyData;
use crate::currency::record::CurrencyRecord;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum Currency {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
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

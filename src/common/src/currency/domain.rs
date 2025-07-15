use crate::currency::command::CurrencyCommand;

#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum Currency {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
}

impl From<CurrencyCommand> for Currency {
    fn from(cmd: CurrencyCommand) -> Self {
        match cmd {
            CurrencyCommand::Eur => Currency::Eur,
            CurrencyCommand::Gbp => Currency::Gbp,
            CurrencyCommand::Usd => Currency::Usd,
            CurrencyCommand::Aud => Currency::Aud,
            CurrencyCommand::Cad => Currency::Cad,
            CurrencyCommand::Nzd => Currency::Nzd,
        }
    }
}

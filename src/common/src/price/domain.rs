use crate::currency::domain::Currency;
use crate::price::command_data::PriceCommandData;
use crate::price::data::PriceData;
use crate::price::record::PriceRecord;
use std::collections::HashMap;
use std::ops::{Add, Deref, Sub};
use strum::{EnumCount, IntoEnumIterator};

type Rate = u64;
const FX_RATE_SCALE: Rate = 1_000_000;

pub trait FxRate {
    fn exchange(
        &self,
        from_currency: Currency,
        to_currency: Currency,
        from_amount: MonetaryAmount,
    ) -> Result<MonetaryAmount, MonetaryAmountOverflowError>;

    fn exchange_all(
        &self,
        from_currency: Currency,
        from_amount: MonetaryAmount,
    ) -> Result<HashMap<Currency, MonetaryAmount>, MonetaryAmountOverflowError> {
        let mut exchanged = HashMap::with_capacity(Currency::COUNT);
        for currency in Currency::iter() {
            exchanged.insert(
                currency,
                self.exchange(from_currency, currency, from_amount)?,
            );
        }
        Ok(exchanged)
    }
}

/// as of 2025-07-15
#[derive(Default)]
pub struct FixedFxRate();

impl FixedFxRate {
    fn get_rate(&self, from: Currency, to: Currency) -> Rate {
        match (from, to) {
            (Currency::Eur, Currency::Eur) => 1_000_000,
            (Currency::Eur, Currency::Usd) => 1_167_000,
            (Currency::Eur, Currency::Gbp) => 867_800,
            (Currency::Eur, Currency::Aud) => 1_778_000,
            (Currency::Eur, Currency::Cad) => 1_597_000,
            (Currency::Eur, Currency::Nzd) => 1_947_000,

            (Currency::Usd, Currency::Eur) => 856_900,
            (Currency::Usd, Currency::Gbp) => 743_700,
            (Currency::Usd, Currency::Aud) => 1_523_000,
            (Currency::Usd, Currency::Cad) => 1_368_000,
            (Currency::Usd, Currency::Nzd) => 1_668_000,
            (Currency::Usd, Currency::Usd) => 1_000_000,

            (Currency::Gbp, Currency::Eur) => 1_152_000,
            (Currency::Gbp, Currency::Usd) => 1_344_000,
            (Currency::Gbp, Currency::Aud) => 2_049_000,
            (Currency::Gbp, Currency::Cad) => 1_840_000,
            (Currency::Gbp, Currency::Nzd) => 2_243_000,
            (Currency::Gbp, Currency::Gbp) => 1_000_000,

            (Currency::Aud, Currency::Eur) => 562_300,
            (Currency::Aud, Currency::Usd) => 656_100,
            (Currency::Aud, Currency::Gbp) => 488_000,
            (Currency::Aud, Currency::Cad) => 898_200,
            (Currency::Aud, Currency::Nzd) => 1_095_000,
            (Currency::Aud, Currency::Aud) => 1_000_000,

            (Currency::Cad, Currency::Eur) => 626_000,
            (Currency::Cad, Currency::Usd) => 730_500,
            (Currency::Cad, Currency::Gbp) => 543_300,
            (Currency::Cad, Currency::Aud) => 1_113_000,
            (Currency::Cad, Currency::Nzd) => 1_219_000,
            (Currency::Cad, Currency::Cad) => 1_000_000,

            (Currency::Nzd, Currency::Eur) => 513_500,
            (Currency::Nzd, Currency::Usd) => 599_300,
            (Currency::Nzd, Currency::Gbp) => 445_700,
            (Currency::Nzd, Currency::Aud) => 913_200,
            (Currency::Nzd, Currency::Cad) => 820_300,
            (Currency::Nzd, Currency::Nzd) => 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, thiserror::Error)]
#[error("Monetary amount overflowed during an internal operation.")]
pub struct MonetaryAmountOverflowError;

impl FxRate for FixedFxRate {
    fn exchange(
        &self,
        from_currency: Currency,
        to_currency: Currency,
        from_amount: MonetaryAmount,
    ) -> Result<MonetaryAmount, MonetaryAmountOverflowError> {
        let rate = self.get_rate(from_currency, to_currency);

        // Half-Up Rounding
        let numerator = from_amount
            .0
            .checked_mul(rate)
            .ok_or(MonetaryAmountOverflowError)?;
        let half = FX_RATE_SCALE / 2;
        let converted = (numerator + half) / FX_RATE_SCALE;

        Ok(MonetaryAmount(converted))
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MonetaryAmount(#[cfg_attr(feature = "test-data", dummy(faker = "0..=1000000000"))] u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, thiserror::Error)]
#[error("Monetary amount cannot be negative.")]
pub struct NegativeMonetaryAmountError;

impl Deref for MonetaryAmount {
    type Target = u64;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Add for MonetaryAmount {
    type Output = MonetaryAmount;

    fn add(self, rhs: Self) -> Self::Output {
        MonetaryAmount(self.0 + rhs.0)
    }
}

impl Sub for MonetaryAmount {
    type Output = Result<MonetaryAmount, NegativeMonetaryAmountError>;

    fn sub(self, rhs: Self) -> Self::Output {
        if self.0 < rhs.0 {
            Err(NegativeMonetaryAmountError)
        } else {
            Ok(MonetaryAmount(self.0 - rhs.0))
        }
    }
}

impl From<u8> for MonetaryAmount {
    fn from(amount: u8) -> Self {
        MonetaryAmount(amount as u64)
    }
}

impl From<u16> for MonetaryAmount {
    fn from(amount: u16) -> Self {
        MonetaryAmount(amount as u64)
    }
}

impl From<u32> for MonetaryAmount {
    fn from(amount: u32) -> Self {
        MonetaryAmount(amount as u64)
    }
}

impl From<u64> for MonetaryAmount {
    fn from(amount: u64) -> Self {
        MonetaryAmount(amount)
    }
}

impl From<MonetaryAmount> for u64 {
    fn from(price: MonetaryAmount) -> Self {
        price.0
    }
}

#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub monetary_amount: MonetaryAmount,
    pub currency: Currency,
}

impl Price {
    pub fn new(monetary_amount: MonetaryAmount, currency: Currency) -> Self {
        Price {
            monetary_amount,
            currency,
        }
    }

    pub fn into_exchanged(
        self,
        fx_rate: &impl FxRate,
        currency: Currency,
    ) -> Result<Price, MonetaryAmountOverflowError> {
        let exchanged = Price {
            monetary_amount: fx_rate.exchange(
                self.currency,
                self.currency,
                self.monetary_amount,
            )?,
            currency,
        };
        Ok(exchanged)
    }

    pub fn exchanged(
        &mut self,
        fx_rate: &impl FxRate,
        currency: Currency,
    ) -> Result<(), MonetaryAmountOverflowError> {
        self.monetary_amount =
            fx_rate.exchange(self.currency, self.currency, self.monetary_amount)?;
        self.currency = currency;
        Ok(())
    }
}

impl From<PriceData> for Price {
    fn from(data: PriceData) -> Self {
        Price {
            monetary_amount: data.amount.into(),
            currency: data.currency.into(),
        }
    }
}

impl From<PriceCommandData> for Price {
    fn from(command_data: PriceCommandData) -> Self {
        Price {
            monetary_amount: command_data.amount.into(),
            currency: command_data.currency.into(),
        }
    }
}

impl From<PriceRecord> for Price {
    fn from(record: PriceRecord) -> Self {
        Price {
            monetary_amount: record.amount.into(),
            currency: record.currency.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::currency::domain::Currency;
    use crate::price::domain::{FxRate, MonetaryAmount, MonetaryAmountOverflowError, Price};

    struct DummyFxRate;
    impl FxRate for DummyFxRate {
        fn exchange(
            &self,
            _: Currency,
            _: Currency,
            from_amount: MonetaryAmount,
        ) -> Result<MonetaryAmount, MonetaryAmountOverflowError> {
            Ok(MonetaryAmount(from_amount.0 * 2))
        }
    }

    #[test]
    fn should_into_exchanged() {
        let price = Price {
            monetary_amount: MonetaryAmount(500),
            currency: Currency::Eur,
        };

        let exchanged = price.into_exchanged(&DummyFxRate, Currency::Gbp);

        assert_eq!(1000, exchanged.unwrap().monetary_amount.0);
    }

    #[test]
    fn should_exchange() {
        let mut price = Price {
            monetary_amount: MonetaryAmount(500),
            currency: Currency::Eur,
        };

        let res = price.exchanged(&DummyFxRate, Currency::Gbp);

        assert!(res.is_ok());
        assert_eq!(1000, price.monetary_amount.0);
    }
}

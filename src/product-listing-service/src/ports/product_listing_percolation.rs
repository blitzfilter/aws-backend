use super::ProductListingSearchFilterMatchSource;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use money::Currency;
use money::Price;
use product_listing_core::product_listing::ProductListingPriceValuationBasis;
use time::OffsetDateTime;

/// Complete, closed-world display prices used by one temporary percolation input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductListingPricesByCurrency {
    eur: u64,
    gbp: u64,
    usd: u64,
    aud: u64,
    cad: u64,
    nzd: u64,
    cny: u64,
    brl: u64,
    pln: u64,
    r#try: u64,
    jpy: u64,
    czk: u64,
    rub: u64,
    aed: u64,
    sar: u64,
    hkd: u64,
    sgd: u64,
    chf: u64,
    zar: u64,
}

impl ProductListingPricesByCurrency {
    pub fn convert_all(
        snapshot: &FxRateSnapshot,
        source_price: Price,
    ) -> Result<Self, FxRateSnapshotError> {
        let amount_in = |currency| {
            snapshot
                .convert(source_price, currency, RoundingMode::HalfUp)
                .map(|price| u64::from(price.monetary_amount))
        };

        Ok(Self {
            eur: amount_in(Currency::Eur)?,
            gbp: amount_in(Currency::Gbp)?,
            usd: amount_in(Currency::Usd)?,
            aud: amount_in(Currency::Aud)?,
            cad: amount_in(Currency::Cad)?,
            nzd: amount_in(Currency::Nzd)?,
            cny: amount_in(Currency::Cny)?,
            brl: amount_in(Currency::Brl)?,
            pln: amount_in(Currency::Pln)?,
            r#try: amount_in(Currency::Try)?,
            jpy: amount_in(Currency::Jpy)?,
            czk: amount_in(Currency::Czk)?,
            rub: amount_in(Currency::Rub)?,
            aed: amount_in(Currency::Aed)?,
            sar: amount_in(Currency::Sar)?,
            hkd: amount_in(Currency::Hkd)?,
            sgd: amount_in(Currency::Sgd)?,
            chf: amount_in(Currency::Chf)?,
            zar: amount_in(Currency::Zar)?,
        })
    }

    pub fn amount_in(self, currency: Currency) -> u64 {
        match currency {
            Currency::Eur => self.eur,
            Currency::Gbp => self.gbp,
            Currency::Usd => self.usd,
            Currency::Aud => self.aud,
            Currency::Cad => self.cad,
            Currency::Nzd => self.nzd,
            Currency::Cny => self.cny,
            Currency::Brl => self.brl,
            Currency::Pln => self.pln,
            Currency::Try => self.r#try,
            Currency::Jpy => self.jpy,
            Currency::Czk => self.czk,
            Currency::Rub => self.rub,
            Currency::Aed => self.aed,
            Currency::Sar => self.sar,
            Currency::Hkd => self.hkd,
            Currency::Sgd => self.sgd,
            Currency::Chf => self.chf,
            Currency::Zar => self.zar,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingPercolationValuation {
    pub basis: ProductListingPriceValuationBasis,
    pub fx_rate_id: FxRateId,
    pub effective_at: OffsetDateTime,
    pub prices: ProductListingPricesByCurrency,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingPercolationInput {
    pub source: ProductListingSearchFilterMatchSource,
    /// Absent only when the ProductListing has no native main price.
    pub valuation: Option<ProductListingPercolationValuation>,
}

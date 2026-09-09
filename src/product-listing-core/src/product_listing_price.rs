use money::Price;

/// Explicit asking-price assertion for a ProductListing.
///
/// `None` around this value means the source made no asking-price assertion.
#[cfg_attr(feature = "test-data", derive(fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductListingPrice {
    Monetary(Price),
    OnRequest,
}

impl ProductListingPrice {
    pub const fn monetary(self) -> Option<Price> {
        match self {
            Self::Monetary(price) => Some(price),
            Self::OnRequest => None,
        }
    }

    pub const fn is_on_request(self) -> bool {
        matches!(self, Self::OnRequest)
    }
}

impl From<Price> for ProductListingPrice {
    fn from(value: Price) -> Self {
        Self::Monetary(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use money::{Currency, MonetaryAmount};

    #[test]
    fn should_expose_monetary_and_on_request_helpers() {
        let price = Price::new(MonetaryAmount::from(12_000_u64), Currency::Eur);

        assert_eq!(Some(price), ProductListingPrice::from(price).monetary());
        assert!(!ProductListingPrice::from(price).is_on_request());
        assert_eq!(None, ProductListingPrice::OnRequest.monetary());
        assert!(ProductListingPrice::OnRequest.is_on_request());
    }
}

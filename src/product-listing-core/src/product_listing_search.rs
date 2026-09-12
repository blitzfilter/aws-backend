use crate::listing_availability::ListingAvailability;
use crate::listing_orderability::ListingOrderability;
use crate::product_listing_id::ProductListingId;

use domain_primitives::query::any_of_query::AnyOfQuery;
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use listing_source_core::ListingSourceId;
use localization::Language;
use money::Currency;
use money::MonetaryAmount;
use serde_fields::SerdeField;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EnhancedSearchDescription(String);

#[derive(Debug, thiserror::Error, Clone, Copy, PartialEq, Eq)]
pub enum EnhancedSearchDescriptionError {
    #[error("enhanced search description must not be blank")]
    Blank,
}

impl EnhancedSearchDescription {
    const MAX_LENGTH: usize = 1000;

    fn canonicalize(value: &str) -> Result<String, EnhancedSearchDescriptionError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(EnhancedSearchDescriptionError::Blank);
        }
        if value.len() <= Self::MAX_LENGTH {
            return Ok(value.to_owned());
        }
        let mut end = Self::MAX_LENGTH - 3;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        Ok(format!("{}...", &value[..end]))
    }
}

impl TryFrom<&str> for EnhancedSearchDescription {
    type Error = EnhancedSearchDescriptionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::canonicalize(value).map(Self)
    }
}

impl TryFrom<String> for EnhancedSearchDescription {
    type Error = EnhancedSearchDescriptionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl AsRef<str> for EnhancedSearchDescription {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for EnhancedSearchDescription {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl std::fmt::Display for EnhancedSearchDescription {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<EnhancedSearchDescription> for String {
    fn from(value: EnhancedSearchDescription) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListingAvailabilityQuery {
    pub any_of: AnyOfQuery<ListingAvailability>,
    pub orderability: AnyOfQuery<ListingOrderability>,
    pub include_unspecified: bool,
}

#[derive(Debug, Clone, PartialEq, Default, SerdeField)]
pub struct ProductListingSearch {
    pub language: Language,
    pub currency: Currency,
    pub product_listing_query: Vec<TextQuery<1>>,
    pub enhanced_search_description: Option<EnhancedSearchDescription>,
    pub exclude_product_listing_id_query: AnyOfQuery<ProductListingId>,
    pub listing_source_id_query: AnyOfQuery<ListingSourceId>,
    pub exclude_listing_source_id_query: AnyOfQuery<ListingSourceId>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub availability_query: Option<ListingAvailabilityQuery>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
    pub lot_bidding_opens_query: Option<RangeQuery<OffsetDateTime>>,
    pub lot_scheduled_closes_query: Option<RangeQuery<OffsetDateTime>>,
}

impl ProductListingSearch {
    pub fn new(language: Language, currency: Currency) -> Self {
        Self {
            language,
            currency,
            product_listing_query: Vec::new(),
            enhanced_search_description: None,
            exclude_product_listing_id_query: AnyOfQuery::default(),
            listing_source_id_query: AnyOfQuery::default(),
            exclude_listing_source_id_query: AnyOfQuery::default(),
            price_query: None,
            availability_query: None,
            created_query: None,
            updated_query: None,
            lot_bidding_opens_query: None,
            lot_scheduled_closes_query: None,
        }
    }

    pub fn with_product_listing_query(mut self, product_listing_query: TextQuery<1>) -> Self {
        self.product_listing_query.push(product_listing_query);
        self
    }

    pub fn with_enhanced_search_description(
        mut self,
        enhanced_search_description: EnhancedSearchDescription,
    ) -> Self {
        self.enhanced_search_description = Some(enhanced_search_description);
        self
    }

    pub fn with_exclude_product_listing_id_query(
        mut self,
        exclude_product_listing_id_query: AnyOfQuery<ProductListingId>,
    ) -> Self {
        self.exclude_product_listing_id_query = exclude_product_listing_id_query;
        self
    }

    pub fn with_listing_source_id_query(
        mut self,
        listing_source_id_query: AnyOfQuery<ListingSourceId>,
    ) -> Self {
        self.listing_source_id_query = listing_source_id_query;
        self
    }

    pub fn with_exclude_listing_source_id_query(
        mut self,
        exclude_listing_source_id_query: AnyOfQuery<ListingSourceId>,
    ) -> Self {
        self.exclude_listing_source_id_query = exclude_listing_source_id_query;
        self
    }

    pub fn with_price_query(mut self, price_query: RangeQuery<MonetaryAmount>) -> Self {
        self.price_query = Some(price_query);
        self
    }

    pub fn with_availability_query(mut self, availability_query: ListingAvailabilityQuery) -> Self {
        self.availability_query = Some(availability_query);
        self
    }

    pub fn with_created_query(mut self, created_query: RangeQuery<OffsetDateTime>) -> Self {
        self.created_query = Some(created_query);
        self
    }

    pub fn with_updated_query(mut self, updated_query: RangeQuery<OffsetDateTime>) -> Self {
        self.updated_query = Some(updated_query);
        self
    }

    pub fn with_lot_bidding_opens_query(
        mut self,
        lot_bidding_opens_query: RangeQuery<OffsetDateTime>,
    ) -> Self {
        self.lot_bidding_opens_query = Some(lot_bidding_opens_query);
        self
    }

    pub fn with_lot_scheduled_closes_query(
        mut self,
        lot_scheduled_closes_query: RangeQuery<OffsetDateTime>,
    ) -> Self {
        self.lot_scheduled_closes_query = Some(lot_scheduled_closes_query);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::query::range_query::RangeQuery;
    use std::collections::HashSet;

    #[test]
    fn should_create_product_listing_search_with_language_and_currency() {
        let search = ProductListingSearch::new(Language::En, Currency::Eur);

        assert_eq!(Language::En, search.language);
        assert_eq!(Currency::Eur, search.currency);
        assert!(search.product_listing_query.is_empty());
    }

    #[test]
    fn should_set_builder_fields() {
        let product_listing_id = ProductListingId::new();
        let listing_source_id = ListingSourceId::new();
        let excluded_listing_source_id = ListingSourceId::new();
        let search = ProductListingSearch::new(Language::En, Currency::Usd)
            .with_product_listing_query(text_query("vase"))
            .with_enhanced_search_description(
                EnhancedSearchDescription::try_from("bronze").unwrap(),
            )
            .with_exclude_product_listing_id_query(HashSet::from([product_listing_id]).into())
            .with_listing_source_id_query(HashSet::from([listing_source_id]).into())
            .with_exclude_listing_source_id_query(
                HashSet::from([excluded_listing_source_id]).into(),
            )
            .with_availability_query(ListingAvailabilityQuery {
                any_of: HashSet::from([ListingAvailability::Available]).into(),
                ..Default::default()
            })
            .with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(10_u64)),
                max: Some(MonetaryAmount::from(20_u64)),
            });

        assert_eq!(1, search.product_listing_query.len());
        assert!(search.enhanced_search_description.is_some());
        assert!(
            search
                .exclude_product_listing_id_query
                .contains(&product_listing_id)
        );
        assert!(search.listing_source_id_query.contains(&listing_source_id));
        assert!(
            search
                .exclude_listing_source_id_query
                .contains(&excluded_listing_source_id)
        );
        assert!(
            search
                .availability_query
                .as_ref()
                .is_some_and(|query| query.any_of.contains(&ListingAvailability::Available))
        );
        assert!(search.price_query.is_some());
    }

    #[test]
    fn should_canonicalize_enhanced_search_description() {
        let description = EnhancedSearchDescription::try_from("  bronze  ").unwrap();
        let truncated = EnhancedSearchDescription::try_from("a".repeat(1200)).unwrap();

        assert_eq!("bronze", description.as_ref());
        assert_eq!(1000, truncated.as_ref().len());
        assert!(matches!(
            EnhancedSearchDescription::try_from(" \n\t "),
            Err(EnhancedSearchDescriptionError::Blank)
        ));
    }

    fn text_query(value: &str) -> TextQuery<1> {
        match TextQuery::try_from(value) {
            Ok(query) => query,
            Err(error) => panic!("invalid test text query: {error}"),
        }
    }
}

#[cfg(feature = "test-data")]
pub mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for ProductListingSearch {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            ProductListingSearch {
                language: config.fake_with_rng(rng),
                currency: config.fake_with_rng(rng),
                product_listing_query: config.fake_with_rng(rng),
                enhanced_search_description: None,
                exclude_product_listing_id_query: config.fake_with_rng(rng),
                listing_source_id_query: AnyOfQuery::default(),
                exclude_listing_source_id_query: AnyOfQuery::default(),
                price_query: config.fake_with_rng(rng),
                availability_query: None,
                created_query: fake_range_query_datetime(config, rng),
                updated_query: fake_range_query_datetime(config, rng),
                lot_bidding_opens_query: fake_range_query_datetime(config, rng),
                lot_scheduled_closes_query: fake_range_query_datetime(config, rng),
            }
        }
    }

    pub fn fake_range_query_datetime<R: fake::RngExt + ?Sized>(
        config: &Faker,
        rng: &mut R,
    ) -> Option<RangeQuery<OffsetDateTime>> {
        if config.fake_with_rng(rng) {
            None
        } else {
            let min = if config.fake_with_rng(rng) {
                Some(OffsetDateTime::now_utc())
            } else {
                None
            };
            let max = if config.fake_with_rng(rng) {
                Some(OffsetDateTime::now_utc())
            } else {
                None
            };
            Some(RangeQuery { min, max })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_product_listing_search() {
            let search = Faker.fake::<ProductListingSearch>();

            assert!(search.listing_source_id_query.is_empty());
            assert!(search.exclude_listing_source_id_query.is_empty());
        }
    }
}

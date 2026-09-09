use crate::source_listing_id::SourceListingId;
use listing_source_core::ListingSourceId;

domain_primitives::object_id_newtype!(ProductListingId, "pl");

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProductListingKey {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
}

impl ProductListingKey {
    pub fn new(listing_source_id: ListingSourceId, source_listing_id: SourceListingId) -> Self {
        Self {
            listing_source_id,
            source_listing_id,
        }
    }
}

impl PartialOrd for ProductListingKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProductListingKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.listing_source_id.as_uuid(), &self.source_listing_id)
            .cmp(&(other.listing_source_id.as_uuid(), &other.source_listing_id))
    }
}

#[cfg(feature = "test-data")]
impl fake::Dummy<fake::Faker> for ProductListingKey {
    fn dummy_with_rng<R: fake::RngExt + ?Sized>(_: &fake::Faker, _: &mut R) -> Self {
        let source_listing_id = SourceListingId::try_from("fake-source-listing-id")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
        Self::new(ListingSourceId::new(), source_listing_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_product_listing_object_id_prefix() {
        let id = ProductListingId::new();

        assert_eq!("pl", ProductListingId::PREFIX);
        assert!(id.to_string().starts_with("pl_"));
    }

    #[cfg(feature = "test-data")]
    #[test]
    fn should_fake_uuid_v7_product_listing_id() {
        use fake::{Fake, Faker};

        let id: ProductListingId = Faker.fake();

        assert_eq!(7, id.as_uuid().get_version_num());
    }

    #[test]
    fn should_create_key_from_listing_source_and_source_listing_ids() {
        let listing_source_id = ListingSourceId::new();
        let source_listing_id = SourceListingId::try_from("source-listing-id")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));

        let key = ProductListingKey::new(listing_source_id, source_listing_id.clone());

        assert_eq!(listing_source_id, key.listing_source_id);
        assert_eq!(source_listing_id, key.source_listing_id);
    }

    #[cfg(feature = "test-data")]
    #[test]
    fn should_fake_product_listing_key() {
        use fake::{Fake, Faker};

        let _ = Faker.fake::<ProductListingKey>();
    }
}

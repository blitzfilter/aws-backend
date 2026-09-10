domain_primitives::object_id_newtype!(ListingSourceId, "ls");

#[cfg(test)]
mod tests {
    use super::ListingSourceId;

    #[test]
    fn should_use_listing_source_object_id_prefix() {
        let id = ListingSourceId::new();

        assert_eq!("ls", ListingSourceId::PREFIX);
        assert!(id.to_string().starts_with("ls_"));
    }

    #[cfg(feature = "test-data")]
    #[test]
    fn should_fake_uuid_v7_listing_source_id() {
        use fake::{Fake, Faker};

        let id: ListingSourceId = Faker.fake();

        assert_eq!(7, id.as_uuid().get_version_num());
    }
}

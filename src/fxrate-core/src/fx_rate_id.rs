domain_primitives::object_id_newtype!(FxRateId, "fx");

#[cfg(test)]
mod tests {
    use super::FxRateId;

    #[test]
    fn should_use_fx_rate_object_id_prefix() {
        let id = FxRateId::new();

        assert_eq!("fx", FxRateId::PREFIX);
        assert!(id.to_string().starts_with("fx_"));
    }

    #[test]
    fn should_roundtrip_valid_uuid_v7_for_storage() -> Result<(), Box<dyn std::error::Error>> {
        let uuid = uuid::uuid!("01890a5d-ac96-774b-bf1d-d5586c639f75");
        let id = FxRateId::try_from(uuid)?;

        assert_eq!(&uuid, id.as_uuid());
        assert_eq!(uuid, id.into_uuid());

        Ok(())
    }

    #[cfg(feature = "test-data")]
    #[test]
    fn should_fake_uuid_v7_fx_rate_id() {
        use fake::{Fake, Faker};

        let id: FxRateId = Faker.fake();

        assert_eq!(7, id.as_uuid().get_version_num());
    }
}

domain_primitives::object_id_newtype!(UserId, "usr");

#[cfg(test)]
mod tests {
    use super::UserId;
    use std::error::Error;
    use uuid::Uuid;

    const UUID_TEXT: &str = "01890a5d-ac96-774b-bf1d-d5586c639f75";
    const TYPE_ID_SUFFIX: &str = "01h455vb4pex5vy7enb1p677vn";

    #[test]
    fn should_use_usr_object_id_contract() -> Result<(), Box<dyn Error>> {
        let uuid = Uuid::parse_str(UUID_TEXT)?;
        let id = UserId::try_from(uuid)?;

        assert_eq!("usr", UserId::PREFIX);
        assert_eq!(format!("usr_{TYPE_ID_SUFFIX}"), id.to_string());
        assert_eq!(uuid, id.into_uuid());
        assert!(UserId::try_from(UUID_TEXT).is_err());
        assert!(UserId::try_from(format!("at_{TYPE_ID_SUFFIX}")).is_err());

        Ok(())
    }

    #[test]
    fn should_generate_uuid_v7_user_id_with_usr_prefix() {
        let id = UserId::new();

        assert_eq!(7, id.as_uuid().get_version_num());
        assert!(id.to_string().starts_with("usr_"));
    }
}

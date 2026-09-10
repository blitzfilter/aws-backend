domain_primitives::object_id_newtype!(OAuthClientId, "oc");

#[cfg(test)]
mod tests {
    use super::OAuthClientId;
    use std::error::Error;
    use uuid::Uuid;

    const UUID_TEXT: &str = "01890a5d-ac96-774b-bf1d-d5586c639f75";
    const TYPE_ID_SUFFIX: &str = "01h455vb4pex5vy7enb1p677vn";

    #[test]
    fn should_use_oc_object_id_contract() -> Result<(), Box<dyn Error>> {
        let uuid = Uuid::parse_str(UUID_TEXT)?;
        let id = OAuthClientId::try_from(uuid)?;

        assert_eq!("oc", OAuthClientId::PREFIX);
        assert_eq!(format!("oc_{TYPE_ID_SUFFIX}"), id.to_string());
        assert_eq!(uuid, id.into_uuid());
        assert!(OAuthClientId::try_from(UUID_TEXT).is_err());
        assert!(OAuthClientId::try_from(format!("usr_{TYPE_ID_SUFFIX}")).is_err());

        Ok(())
    }

    #[test]
    fn should_generate_uuid_v7_oauth_client_id_with_oc_prefix() {
        let id = OAuthClientId::new();

        assert_eq!(7, id.as_uuid().get_version_num());
        assert!(id.to_string().starts_with("oc_"));
    }
}

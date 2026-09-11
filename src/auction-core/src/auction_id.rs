domain_primitives::object_id_newtype!(AuctionId, "auc");

#[cfg(test)]
mod tests {
    use super::AuctionId;
    use domain_primitives::object_id::ObjectIdError;
    use std::str::FromStr;
    use uuid::Uuid;

    const UUID_TEXT: &str = "01890a5d-ac96-774b-bf1d-d5586c639f75";
    const TYPE_ID_SUFFIX: &str = "01h455vb4pex5vy7enb1p677vn";

    #[test]
    fn should_use_auction_object_id_prefix() {
        let id = AuctionId::new();

        assert_eq!("auc", AuctionId::PREFIX);
        assert!(id.to_string().starts_with("auc_"));
        assert_eq!(7, id.as_uuid().get_version_num());
    }

    #[test]
    fn should_strictly_parse_only_canonical_auction_type_ids() -> Result<(), ObjectIdError> {
        let parsed = AuctionId::from_str(&format!("auc_{TYPE_ID_SUFFIX}"))?;

        assert_eq!(UUID_TEXT, parsed.as_uuid().to_string());
        assert!(AuctionId::from_str(UUID_TEXT).is_err());
        assert!(AuctionId::from_str(&format!("pl_{TYPE_ID_SUFFIX}")).is_err());
        assert!(AuctionId::from_str(&format!("AUC_{TYPE_ID_SUFFIX}")).is_err());
        assert!(AuctionId::from_str("auc_01H455vb4pex5vy7enb1p677vn").is_err());
        assert!(AuctionId::from_str("auc_01h455vb4pex5vy7enb1p677v!").is_err());
        Ok(())
    }

    #[test]
    fn should_reject_non_v7_uuid_rehydration() {
        let uuid = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")
            .unwrap_or_else(|error| panic!("valid test UUID: {error}"));

        assert!(AuctionId::try_from(uuid).is_err());
    }
}

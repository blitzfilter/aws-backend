domain_primitives::object_id_newtype!(PartnershipId, "psh");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_psh_prefix_for_partnership_id()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let id = PartnershipId::try_from("psh_01h455vb4pex5vy7enb1p677vn")?;

        assert_eq!("psh", PartnershipId::PREFIX);
        assert_eq!("psh_01h455vb4pex5vy7enb1p677vn", id.to_string());
        assert_eq!(7, id.as_uuid().get_version_num());
        Ok(())
    }
}

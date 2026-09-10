domain_primitives::object_id_newtype!(PartnershipApplicationId, "pa");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_pa_prefix_for_partnership_application_id()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let id = PartnershipApplicationId::try_from("pa_01h455vb4pex5vy7enb1p677vn")?;

        assert_eq!("pa", PartnershipApplicationId::PREFIX);
        assert_eq!("pa_01h455vb4pex5vy7enb1p677vn", id.to_string());
        assert_eq!(7, id.as_uuid().get_version_num());
        Ok(())
    }
}

domain_primitives::object_id_newtype!(UserSearchFilterId, "sf");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_sf_prefix_for_user_search_filter_id()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let id = UserSearchFilterId::try_from("sf_01h455vb4pex5vy7enb1p677vn")?;

        assert_eq!("sf", UserSearchFilterId::PREFIX);
        assert_eq!("sf_01h455vb4pex5vy7enb1p677vn", id.to_string());
        assert_eq!(7, id.as_uuid().get_version_num());
        Ok(())
    }
}

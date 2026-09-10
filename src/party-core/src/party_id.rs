domain_primitives::object_id_newtype!(PartyId, "pty");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_pty_prefix_for_party_id()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let id = PartyId::try_from("pty_01h455vb4pex5vy7enb1p677vn")?;

        assert_eq!("pty", PartyId::PREFIX);
        assert_eq!("pty_01h455vb4pex5vy7enb1p677vn", id.to_string());
        assert_eq!(7, id.as_uuid().get_version_num());
        Ok(())
    }
}

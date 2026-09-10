domain_primitives::object_id_newtype!(NotificationId, "ntf");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_ntf_prefix_for_notification_id()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let id = NotificationId::try_from("ntf_01h455vb4pex5vy7enb1p677vn")?;

        assert_eq!("ntf", NotificationId::PREFIX);
        assert_eq!("ntf_01h455vb4pex5vy7enb1p677vn", id.to_string());
        assert_eq!(7, id.as_uuid().get_version_num());
        Ok(())
    }
}

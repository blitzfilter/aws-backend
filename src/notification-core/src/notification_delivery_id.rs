domain_primitives::object_id_newtype!(NotificationDeliveryId, "nd");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_nd_prefix_for_notification_delivery_id()
    -> Result<(), domain_primitives::object_id::ObjectIdError> {
        let id = NotificationDeliveryId::try_from("nd_01h455vb4pex5vy7enb1p677vn")?;

        assert_eq!("nd", NotificationDeliveryId::PREFIX);
        assert_eq!("nd_01h455vb4pex5vy7enb1p677vn", id.to_string());
        assert_eq!(7, id.as_uuid().get_version_num());
        Ok(())
    }
}

use application::error::{BoxError, box_error};

#[derive(Debug, thiserror::Error)]
#[error("persisted {kind} UUID is invalid")]
pub(crate) struct PersistedObjectIdError {
    kind: &'static str,
    #[source]
    source: BoxError,
}

pub(crate) fn try_from_uuid<T>(
    value: uuid::Uuid,
    kind: &'static str,
) -> Result<T, PersistedObjectIdError>
where
    T: TryFrom<uuid::Uuid>,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    T::try_from(value).map_err(|source| PersistedObjectIdError {
        kind,
        source: box_error(source),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::product_listing_id::ProductListingId;

    #[test]
    fn should_round_trip_native_storage_uuid() {
        let expected = ProductListingId::new();
        let actual = try_from_uuid(expected.into_uuid(), "ProductListing ID")
            .unwrap_or_else(|error| panic!("valid native storage UUID: {error}"));

        assert_eq!(expected, actual);
    }

    #[test]
    fn should_preserve_strict_object_id_error_for_invalid_v4_storage_uuid() {
        let invalid_uuid = "67e55044-10b1-426f-9247-bb680e5fe0c8"
            .parse::<uuid::Uuid>()
            .unwrap_or_else(|error| panic!("valid UUIDv4 fixture: {error}"));
        let error =
            try_from_uuid::<ProductListingId>(invalid_uuid, "ProductListing ID").unwrap_err();

        assert_eq!(
            "persisted ProductListing ID UUID is invalid",
            error.to_string()
        );
        assert!(std::error::Error::source(&error).is_some());
    }
}

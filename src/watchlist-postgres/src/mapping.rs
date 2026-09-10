use domain_primitives::versioned::Versioned;
use product_listing_core::product_listing_id::ProductListingId;
use sqlx::FromRow;
use user_core::user_id::UserId;
use watchlist_core::WatchlistProductListing;
use watchlist_core::WatchlistState;
use watchlist_service::ports::{
    VersionedWatchlistProductListing, WatchlistProductListingView, WatchlistReadError,
    WatchlistRepositoryError, WatchlistStorageVersion,
};

#[derive(FromRow)]
pub(crate) struct WatchlistRepositoryRow {
    pub user_id: uuid::Uuid,
    pub product_listing_id: uuid::Uuid,
    pub notifications: bool,
    pub state: String,
    pub version: i64,
}

#[derive(FromRow)]
pub(crate) struct WatchlistViewRow {
    pub user_id: uuid::Uuid,
    pub product_listing_id: uuid::Uuid,
    pub notifications: bool,
    pub state: String,
    pub created: time::OffsetDateTime,
    pub updated: time::OffsetDateTime,
}

impl TryFrom<WatchlistRepositoryRow> for VersionedWatchlistProductListing {
    type Error = WatchlistRepositoryError;

    fn try_from(row: WatchlistRepositoryRow) -> Result<Self, Self::Error> {
        let version = WatchlistStorageVersion::try_from(row.version)
            .map_err(|_| WatchlistRepositoryError::InvalidPersistedState)?;
        let entry = WatchlistProductListing::rehydrate(
            UserId::try_from(row.user_id)
                .map_err(|_| WatchlistRepositoryError::InvalidPersistedState)?,
            ProductListingId::try_from(row.product_listing_id)
                .map_err(|_| WatchlistRepositoryError::InvalidPersistedState)?,
            row.notifications,
            parse_state_repository(&row.state)?,
        );
        Ok(Versioned::new(entry, version))
    }
}

impl TryFrom<WatchlistViewRow> for WatchlistProductListingView {
    type Error = WatchlistReadError;

    fn try_from(row: WatchlistViewRow) -> Result<Self, Self::Error> {
        Ok(Self {
            user_id: UserId::try_from(row.user_id)
                .map_err(|_| WatchlistReadError::InvalidPersistedState)?,
            product_listing_id: ProductListingId::try_from(row.product_listing_id)
                .map_err(|_| WatchlistReadError::InvalidPersistedState)?,
            notifications: row.notifications,
            state: parse_state_read(&row.state)?,
            created: row.created,
            updated: row.updated,
        })
    }
}

fn parse_state(value: &str) -> Option<WatchlistState> {
    WatchlistState::from_code(value)
}

fn parse_state_repository(value: &str) -> Result<WatchlistState, WatchlistRepositoryError> {
    parse_state(value).ok_or(WatchlistRepositoryError::InvalidPersistedState)
}

fn parse_state_read(value: &str) -> Result<WatchlistState, WatchlistReadError> {
    parse_state(value).ok_or(WatchlistReadError::InvalidPersistedState)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn should_parse_each_canonical_state() {
        for expected in WatchlistState::iter() {
            assert_eq!(Some(expected), parse_state(expected.as_str()));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_states() {
        for value in ["bad", "active"] {
            assert!(matches!(
                parse_state_repository(value),
                Err(WatchlistRepositoryError::InvalidPersistedState)
            ));
            assert!(matches!(
                parse_state_read(value),
                Err(WatchlistReadError::InvalidPersistedState)
            ));
        }
    }

    #[test]
    fn should_round_trip_v7_ids_from_storage_rows() {
        let user_id = UserId::new();
        let product_listing_id = ProductListingId::new();
        let row = WatchlistRepositoryRow {
            user_id: user_id.into_uuid(),
            product_listing_id: product_listing_id.into_uuid(),
            notifications: true,
            state: "ACTIVE".to_owned(),
            version: 1,
        };

        let persisted = VersionedWatchlistProductListing::try_from(row)
            .unwrap_or_else(|error| panic!("valid storage row failed: {error}"));

        assert_eq!(user_id, persisted.value.user_id());
        assert_eq!(product_listing_id, persisted.value.product_listing_id());
    }

    #[test]
    fn should_reject_v4_ids_from_storage_rows() {
        let repository_row = WatchlistRepositoryRow {
            user_id: uuid::Uuid::new_v4(),
            product_listing_id: ProductListingId::new().into_uuid(),
            notifications: true,
            state: "ACTIVE".to_owned(),
            version: 1,
        };
        let view_row = WatchlistViewRow {
            user_id: UserId::new().into_uuid(),
            product_listing_id: uuid::Uuid::new_v4(),
            notifications: true,
            state: "ACTIVE".to_owned(),
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        };

        assert!(matches!(
            VersionedWatchlistProductListing::try_from(repository_row),
            Err(WatchlistRepositoryError::InvalidPersistedState)
        ));
        assert!(matches!(
            WatchlistProductListingView::try_from(view_row),
            Err(WatchlistReadError::InvalidPersistedState)
        ));
    }

    #[test]
    fn should_reject_zero_or_negative_repository_version() {
        for version in [0, -1] {
            let row = WatchlistRepositoryRow {
                user_id: uuid::Uuid::now_v7(),
                product_listing_id: uuid::Uuid::now_v7(),
                notifications: true,
                state: "ACTIVE".to_owned(),
                version,
            };

            assert!(matches!(
                VersionedWatchlistProductListing::try_from(row),
                Err(WatchlistRepositoryError::InvalidPersistedState)
            ));
        }
    }
}

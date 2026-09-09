use application::error::box_error;
use domain_primitives::event_id::EventId;
use platform_postgres::SqlxTransaction;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    SearchFilterMatchNotificationSource, SearchFilterMatchNotificationSourceReadError,
    SearchFilterMatchNotificationSourceReader, SearchFilterMatchNotificationSourceReaderFactory,
};
use sqlx::FromRow;
use time::OffsetDateTime;
use user_core::user_id::UserId;

use crate::mapping::name;

#[derive(Debug, Clone, Default)]
pub struct SqlxSearchFilterMatchNotificationSourceReaderFactory;

struct SqlxSearchFilterMatchNotificationSourceReader<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl SearchFilterMatchNotificationSourceReaderFactory<SqlxTransaction>
    for SqlxSearchFilterMatchNotificationSourceReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl SearchFilterMatchNotificationSourceReader + 'tx {
        SqlxSearchFilterMatchNotificationSourceReader { tx }
    }
}

#[derive(Debug, FromRow)]
struct SearchFilterMatchNotificationSourceRow {
    user_id: uuid::Uuid,
    user_search_filter_id: uuid::Uuid,
    product_listing_id: uuid::Uuid,
    origin_event_id: uuid::Uuid,
    created: OffsetDateTime,
    user_search_filter_name: String,
    external_delivery_requested: bool,
}

#[async_trait::async_trait]
impl SearchFilterMatchNotificationSourceReader
    for SqlxSearchFilterMatchNotificationSourceReader<'_>
{
    async fn find_source(
        &mut self,
        user_id: UserId,
        search_filter_id: UserSearchFilterId,
        product_listing_id: ProductListingId,
        origin_event_id: EventId,
    ) -> Result<
        Option<SearchFilterMatchNotificationSource>,
        SearchFilterMatchNotificationSourceReadError,
    > {
        let search_filter_id = search_filter_id.into_uuid();
        let row = sqlx::query_as::<_, SearchFilterMatchNotificationSourceRow>(
            r#"
            SELECT
                matched.user_id,
                matched.user_search_filter_id,
                matched.product_listing_id,
                matched.origin_event_id,
                matched.created,
                COALESCE(matched.user_search_filter_name, filter.name) AS user_search_filter_name,
                filter.notifications AS external_delivery_requested
            FROM search_filter_matches matched
            JOIN search_filters filter
                ON filter.user_search_filter_id = matched.user_search_filter_id
            WHERE matched.user_id = $1
                AND matched.user_search_filter_id = $2
                AND matched.product_listing_id = $3
                AND matched.origin_event_id = $4
            "#,
        )
        .bind(user_id.into_uuid())
        .bind(search_filter_id)
        .bind(product_listing_id.into_uuid())
        .bind(origin_event_id.into_uuid())
        .fetch_optional(self.tx.connection())
        .await
        .map_err(
            |source| SearchFilterMatchNotificationSourceReadError::ReadFailed {
                source: box_error(source),
            },
        )?;

        row.map(|row| {
            Ok(SearchFilterMatchNotificationSource {
                user_id: UserId::try_from(row.user_id).map_err(|source| {
                    SearchFilterMatchNotificationSourceReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                search_filter_id: UserSearchFilterId::try_from(row.user_search_filter_id).map_err(
                    |source| SearchFilterMatchNotificationSourceReadError::InvalidPersistedState {
                        source: box_error(source),
                    },
                )?,
                search_filter_name: name(row.user_search_filter_name).map_err(|source| {
                    SearchFilterMatchNotificationSourceReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                product_listing_id: ProductListingId::try_from(row.product_listing_id).map_err(
                    |source| SearchFilterMatchNotificationSourceReadError::InvalidPersistedState {
                        source: box_error(source),
                    },
                )?,
                origin_event_id: EventId::try_from(row.origin_event_id).map_err(|source| {
                    SearchFilterMatchNotificationSourceReadError::InvalidPersistedState {
                        source: box_error(source),
                    }
                })?,
                matched_at: row.created,
                external_delivery_requested: row.external_delivery_requested,
            })
        })
        .transpose()
    }
}

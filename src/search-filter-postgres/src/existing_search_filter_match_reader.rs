use application::error::box_error;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_service::ports::{
    ExistingSearchFilterMatchReadError, ExistingSearchFilterMatchReader,
};
use sqlx::PgPool;
use std::collections::HashSet;

#[derive(Clone)]
pub struct SqlxExistingSearchFilterMatchReader {
    pool: PgPool,
}
impl SqlxExistingSearchFilterMatchReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ExistingSearchFilterMatchReader for SqlxExistingSearchFilterMatchReader {
    async fn find_existing_product_listing_ids(
        &self,
        search_filter_id: UserSearchFilterId,
        product_listing_ids: &[ProductListingId],
    ) -> Result<HashSet<ProductListingId>, ExistingSearchFilterMatchReadError> {
        if product_listing_ids.is_empty() {
            return Ok(HashSet::new());
        }
        let search_filter_id = search_filter_id.into_uuid();
        let ids = product_listing_ids
            .iter()
            .copied()
            .map(ProductListingId::into_uuid)
            .collect::<Vec<_>>();
        let existing = sqlx::query_scalar::<_, uuid::Uuid>("SELECT product_listing_id FROM search_filter_matches WHERE user_search_filter_id = $1 AND product_listing_id = ANY($2)")
            .bind(search_filter_id).bind(ids).fetch_all(&self.pool).await
            .map_err(|source| ExistingSearchFilterMatchReadError::ReadFailed { source: box_error(source) })?;
        existing
            .into_iter()
            .map(|id| {
                ProductListingId::try_from(id).map_err(|source| {
                    ExistingSearchFilterMatchReadError::ReadFailed {
                        source: box_error(source),
                    }
                })
            })
            .collect()
    }
}

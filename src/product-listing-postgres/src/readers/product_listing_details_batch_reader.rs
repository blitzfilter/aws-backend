use super::product_listing_details_reader::{ProductListingDetailsRow, product_details_select};
use application::error::box_error;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    PersonalizedProductListingDetailsReadModel, ProductListingDetailsBatchReadError,
    ProductListingDetailsBatchReadRequest, ProductListingDetailsBatchReader,
};
use sqlx::{PgPool, Postgres, QueryBuilder};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SqlxProductListingDetailsBatchReader {
    pool: PgPool,
}

impl SqlxProductListingDetailsBatchReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("product details batch SQL query failed")]
struct ProductListingDetailsBatchQuerySqlxError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product details batch row could not map to the read model")]
struct ProductListingDetailsBatchReadModelMappingError {
    #[source]
    source: super::product_listing_details_reader::ProductListingDetailsRowMappingError,
}

impl From<ProductListingDetailsBatchQuerySqlxError> for ProductListingDetailsBatchReadError {
    fn from(source: ProductListingDetailsBatchQuerySqlxError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductListingDetailsBatchReadModelMappingError> for ProductListingDetailsBatchReadError {
    fn from(source: ProductListingDetailsBatchReadModelMappingError) -> Self {
        Self::InvalidReadModel {
            source: box_error(source),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingDetailsBatchReader for SqlxProductListingDetailsBatchReader {
    async fn find_for_user(
        &self,
        request: &ProductListingDetailsBatchReadRequest,
    ) -> Result<
        HashMap<ProductListingId, PersonalizedProductListingDetailsReadModel>,
        ProductListingDetailsBatchReadError,
    > {
        if request.product_listing_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let product_listing_ids = request
            .product_listing_ids
            .iter()
            .copied()
            .map(|id| id.into_uuid())
            .collect::<Vec<_>>();
        let search_filter_id = request.search_filter_id.as_uuid();
        let select = product_details_select(
            r#"
    requested_products AS (
        SELECT DISTINCT requested.product_listing_id
        FROM UNNEST($3::uuid[]) AS requested(product_listing_id)
    ),
    notification_states AS (
        SELECT
            notification.product_listing_id,
            array_agg(
                notification.notification_id
                ORDER BY notification.created DESC, notification.notification_id DESC
            ) AS unseen_notification_ids
        FROM notifications notification
        JOIN requested_products requested
            ON requested.product_listing_id = notification.product_listing_id
        WHERE notification.user_id = $2
            AND notification.seen = false
        GROUP BY notification.product_listing_id
    )
"#,
        )
        .replace(
            "AND matched.product_listing_id = p.product_listing_id",
            "AND matched.product_listing_id = p.product_listing_id AND matched.user_search_filter_id = $4",
        );
        let mut query = QueryBuilder::<Postgres>::new(select);
        query.push(" WHERE p.product_listing_id = ANY($3)");
        let rows = query
            .build_query_as::<ProductListingDetailsRow>()
            .bind(request.language.as_str())
            .bind(request.user_id.as_uuid())
            .bind(product_listing_ids)
            .bind(search_filter_id)
            .fetch_all(&self.pool)
            .await
            .map_err(ProductListingDetailsBatchQuerySqlxError)?;

        rows.into_iter()
            .map(|row| {
                let details = PersonalizedProductListingDetailsReadModel::try_from(row)
                    .map_err(|source| ProductListingDetailsBatchReadModelMappingError { source })?;
                Ok((details.item.product_listing_id, details))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error: ProductListingDetailsBatchReadError =
            ProductListingDetailsBatchQuerySqlxError(sqlx::Error::RowNotFound).into();

        let ProductListingDetailsBatchReadError::QueryFailed { source } = error else {
            panic!("expected batch query failure");
        };
        assert!(
            source
                .downcast_ref::<ProductListingDetailsBatchQuerySqlxError>()
                .is_some()
        );
        assert!(source.source().is_some());
    }

    #[test]
    fn should_map_read_model_failure_without_exposing_row() {
        let error: ProductListingDetailsBatchReadError =
            ProductListingDetailsBatchReadModelMappingError {
                source: crate::readers::product_listing_details_reader::ProductListingDetailsRowMappingError::InvalidValue,
            }
            .into();

        let ProductListingDetailsBatchReadError::InvalidReadModel { source } = error else {
            panic!("expected invalid batch read model");
        };
        assert!(
            source
                .downcast_ref::<ProductListingDetailsBatchReadModelMappingError>()
                .is_some()
        );
    }
}

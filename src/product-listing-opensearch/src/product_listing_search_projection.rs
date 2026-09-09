use crate::product_listing_percolation_document::{
    ProductListingPercolationDocumentError, product_listing_document,
};
use application::error::box_error;
use fxrate_core::FxRateSnapshot;
use opensearch::{IndexParts, OpenSearch, http::StatusCode, params::VersionType};
use platform_opensearch::response::read_response;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{
    ProductListingSearchFilterMatchSource, ProductListingSearchProjection,
    ProductListingSearchProjectionWriteError, ProductListingSearchProjectionWriteOutcome,
};

const DEFAULT_INDEX: &str = "product-listings";

#[derive(Clone)]
pub struct OpenSearchProductListingSearchProjection {
    client: OpenSearch,
    index: String,
}

impl OpenSearchProductListingSearchProjection {
    pub fn new(client: OpenSearch) -> Self {
        Self {
            client,
            index: DEFAULT_INDEX.to_owned(),
        }
    }

    pub fn with_index(client: OpenSearch, index: impl Into<String>) -> Self {
        Self {
            client,
            index: index.into(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingSearchProjection for OpenSearchProductListingSearchProjection {
    async fn upsert(
        &self,
        source: &ProductListingSearchFilterMatchSource,
        sale_snapshot: Option<&FxRateSnapshot>,
    ) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError>
    {
        let version = checked_source_version(source.projection_version)?;
        let document = product_listing_document(source, sale_snapshot).map_err(document_error)?;
        let document_id = document._id().to_string();
        let response = self
            .client
            .index(IndexParts::IndexId(&self.index, &document_id))
            .version(version)
            .version_type(VersionType::External)
            .body(document)
            .send()
            .await
            .map_err(
                |source| ProductListingSearchProjectionWriteError::WriteFailed {
                    source: box_error(source),
                },
            )?;
        write_response_outcome(response, true).await
    }

    async fn delete(
        &self,
        product_listing_id: ProductListingId,
        source_version: i64,
    ) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError>
    {
        let version = checked_source_version(source_version)?;
        let response = self
            .client
            .index(IndexParts::IndexId(
                &self.index,
                &product_listing_id.to_string(),
            ))
            .version(version)
            .version_type(VersionType::External)
            // Physical DELETE forgets its version after index.gc_deletes (default 60s).
            // Keep this content-free document until a strictly newer source replaces it.
            .body(serde_json::json!({
                "productListingId": product_listing_id,
                "projectionDeleted": true,
            }))
            .send()
            .await
            .map_err(
                |source| ProductListingSearchProjectionWriteError::DeleteFailed {
                    source: box_error(source),
                },
            )?;
        write_response_outcome(response, false).await
    }
}

fn checked_source_version(version: i64) -> Result<i64, ProductListingSearchProjectionWriteError> {
    (version >= 1).then_some(version).ok_or_else(|| {
        ProductListingSearchProjectionWriteError::WriteFailed {
            source: application::error::static_error(
                "ProductListing projection version must be positive",
            ),
        }
    })
}

fn document_error(
    source: ProductListingPercolationDocumentError,
) -> ProductListingSearchProjectionWriteError {
    ProductListingSearchProjectionWriteError::WriteFailed {
        source: box_error(source),
    }
}

async fn write_response_outcome(
    response: opensearch::http::response::Response,
    is_write: bool,
) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError> {
    let response = read_response(response).await.map_err(|source| {
        projection_write_error(is_write, application::error::box_error(source))
    })?;
    let status = response.status();
    if let Ok(outcome) = write_outcome(status, is_write) {
        return Ok(outcome);
    }
    Err(projection_write_error(
        is_write,
        application::error::box_error(std::io::Error::other(format!(
            "OpenSearch ProductListing projection returned {status}: {}",
            response.body(),
        ))),
    ))
}

fn write_outcome(
    status: StatusCode,
    is_write: bool,
) -> Result<ProductListingSearchProjectionWriteOutcome, ProductListingSearchProjectionWriteError> {
    if status == StatusCode::CONFLICT {
        return Ok(ProductListingSearchProjectionWriteOutcome::Stale);
    }
    if status.is_success() {
        return Ok(ProductListingSearchProjectionWriteOutcome::Applied);
    }
    Err(projection_write_error(
        is_write,
        application::error::static_error(
            "OpenSearch ProductListing projection returned an unsuccessful status",
        ),
    ))
}

fn projection_write_error(
    is_write: bool,
    source: application::error::BoxError,
) -> ProductListingSearchProjectionWriteError {
    if is_write {
        ProductListingSearchProjectionWriteError::WriteFailed { source }
    } else {
        ProductListingSearchProjectionWriteError::DeleteFailed { source }
    }
}

#[cfg(test)]
#[path = "paused_write.rs"]
mod paused_write;
#[cfg(test)]
#[path = "projection_race_tests.rs"]
mod race_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_non_positive_projection_version() {
        assert!(matches!(
            checked_source_version(0),
            Err(ProductListingSearchProjectionWriteError::WriteFailed { .. })
        ));
    }

    #[test]
    fn should_accept_positive_projection_version() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(42, checked_source_version(42)?);
        Ok(())
    }

    #[test]
    fn should_map_conflict_to_stale_for_writes_and_deletes()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Stale,
            write_outcome(StatusCode::CONFLICT, true)?
        );
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Stale,
            write_outcome(StatusCode::CONFLICT, false)?
        );
        Ok(())
    }

    #[test]
    fn should_map_success_to_applied_for_writes_and_deletes()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Applied,
            write_outcome(StatusCode::CREATED, true)?
        );
        assert_eq!(
            ProductListingSearchProjectionWriteOutcome::Applied,
            write_outcome(StatusCode::OK, false)?
        );
        Ok(())
    }

    #[test]
    fn should_map_unsuccessful_status_to_operation_specific_error() {
        assert!(matches!(
            write_outcome(StatusCode::INTERNAL_SERVER_ERROR, true),
            Err(ProductListingSearchProjectionWriteError::WriteFailed { .. })
        ));
        assert!(matches!(
            write_outcome(StatusCode::BAD_GATEWAY, false),
            Err(ProductListingSearchProjectionWriteError::DeleteFailed { .. })
        ));
    }
}

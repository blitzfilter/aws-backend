use crate::object_id::try_from_uuid;
use product_listing_service::ports::{
    ProductListingEmbedding, ProductListingEmbeddingLookup, ProductListingEmbeddingReadError,
    ProductListingEmbeddingReader, ProductListingEmbeddingReaderFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEmbeddingReaderFactory;

struct SqlxProductListingEmbeddingReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingEmbeddingRow {
    product_listing_id: uuid::Uuid,
    embedding: Option<Vec<f32>>,
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding query failed")]
struct ProductListingEmbeddingQuerySqlxError(#[source] sqlx::Error);

impl TryFrom<ProductListingEmbeddingRow> for ProductListingEmbedding {
    type Error = crate::object_id::PersistedObjectIdError;

    fn try_from(row: ProductListingEmbeddingRow) -> Result<Self, Self::Error> {
        Ok(Self {
            product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")?,
            embedding: row.embedding,
        })
    }
}

impl SqlxProductListingEmbeddingReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEmbeddingReaderFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingEmbeddingReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingEmbeddingReader + 'tx {
        SqlxProductListingEmbeddingReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingReader for SqlxProductListingEmbeddingReader<'_> {
    async fn find_embedding(
        &mut self,
        lookup: &ProductListingEmbeddingLookup,
    ) -> Result<Option<ProductListingEmbedding>, ProductListingEmbeddingReadError> {
        let query = match lookup {
            ProductListingEmbeddingLookup::ById(product_listing_id) => sqlx::query_as::<_, ProductListingEmbeddingRow>(
                "SELECT product_listing_id, embedding FROM product_listings WHERE product_listing_id = $1",
            )
            .bind(product_listing_id.as_uuid()),
            ProductListingEmbeddingLookup::ByTitleSlug(product_listing_title_slug_id) => sqlx::query_as::<_, ProductListingEmbeddingRow>(
                "SELECT p.product_listing_id, p.embedding FROM product_listings p WHERE p.product_listing_title_slug_id = $1",
            )
            .bind(product_listing_title_slug_id.as_ref()),
        };
        let row = query
            .fetch_optional(&mut *self.connection)
            .await
            .map_err(|source| {
                ProductListingEmbeddingReadError::ProductListingEmbeddingQueryFailed {
                    source: Box::new(ProductListingEmbeddingQuerySqlxError(source)),
                }
            })?;
        row.map(TryInto::try_into).transpose().map_err(|source| {
            ProductListingEmbeddingReadError::ProductListingEmbeddingQueryFailed {
                source: Box::new(source),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_map_embedding_row_with_missing_embedding() {
        let product_listing_id = product_listing_core::product_listing_id::ProductListingId::new();
        let embedding = ProductListingEmbedding::try_from(ProductListingEmbeddingRow {
            product_listing_id: product_listing_id.into_uuid(),
            embedding: None,
        })
        .unwrap_or_else(|error| panic!("valid embedding row: {error}"));

        assert_eq!(product_listing_id, embedding.product_listing_id);
        assert!(embedding.embedding.is_none());
    }

    #[test]
    fn should_reject_v4_product_listing_id_in_embedding_row() {
        let invalid_uuid = "67e55044-10b1-426f-9247-bb680e5fe0c8"
            .parse::<uuid::Uuid>()
            .unwrap_or_else(|error| panic!("valid UUIDv4 fixture: {error}"));
        let result = ProductListingEmbedding::try_from(ProductListingEmbeddingRow {
            product_listing_id: invalid_uuid,
            embedding: None,
        });

        assert!(result.is_err());
    }
}

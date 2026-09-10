use crate::readers::product_listing_content_assessment_reader::decode_decision;
use application::error::box_error;

use platform_postgres::SqlxTransaction;
use product_listing_core::{
    content_policy::ContentPolicyDecision, product_listing_id::ProductListingId,
};
use product_listing_service::ports::{
    ProductListingContentAssessmentReadError, ProductListingContentAssessmentSnapshotReader,
    ProductListingContentAssessmentSnapshotReaderFactory,
};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingContentAssessmentSnapshotReaderFactory;

struct SqlxProductListingContentAssessmentSnapshotReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingContentAssessmentSnapshotRow {
    decision: String,
    category: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product content assessment snapshot SQL query failed")]
struct ProductListingContentAssessmentSnapshotQueryError(#[source] sqlx::Error);

impl SqlxProductListingContentAssessmentSnapshotReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingContentAssessmentSnapshotReaderFactory<SqlxTransaction>
    for SqlxProductListingContentAssessmentSnapshotReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingContentAssessmentSnapshotReader + 'tx {
        SqlxProductListingContentAssessmentSnapshotReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingContentAssessmentSnapshotReader
    for SqlxProductListingContentAssessmentSnapshotReader<'_>
{
    async fn find_current_for_product_listing(
        &mut self,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ContentPolicyDecision>, ProductListingContentAssessmentReadError> {
        let row = sqlx::query_as::<_, ProductListingContentAssessmentSnapshotRow>(
            r#"
            SELECT assessment.decision, assessment.category
            FROM product_listings product
            JOIN product_listing_content_assessments assessment
              ON assessment.product_listing_id = product.product_listing_id
             AND assessment.source_event_id = product.content_source_event_id
            WHERE product.product_listing_id = $1
            "#,
        )
        .bind(product_listing_id.as_uuid())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(
            |source| ProductListingContentAssessmentReadError::QueryFailed {
                source: box_error(ProductListingContentAssessmentSnapshotQueryError(source)),
            },
        )?;

        row.map(|row| decode_decision(&row.decision, row.category.as_deref()))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_sqlx_query_source() {
        let error = ProductListingContentAssessmentReadError::QueryFailed {
            source: box_error(ProductListingContentAssessmentSnapshotQueryError(
                sqlx::Error::RowNotFound,
            )),
        };

        let ProductListingContentAssessmentReadError::QueryFailed { source } = error else {
            panic!("expected query failure");
        };
        assert!(
            source
                .downcast_ref::<ProductListingContentAssessmentSnapshotQueryError>()
                .is_some()
        );
    }
}

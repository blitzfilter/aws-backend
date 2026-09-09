use crate::object_id::try_from_uuid;
use application::error::{box_error, static_error};
use product_listing_core::{
    content_policy::{ContentPolicyDecision, SensitiveContentCategory},
    product_listing_id::ProductListingId,
};
use product_listing_service::ports::{
    ProductListingContentAssessment, ProductListingContentAssessmentReadError,
    ProductListingContentAssessmentReader,
};
use sqlx::PgPool;
use std::collections::HashMap;

#[derive(Clone)]
pub struct SqlxProductListingContentAssessmentReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingContentAssessmentRow {
    product_listing_id: uuid::Uuid,
    source_event_id: uuid::Uuid,
    decision: String,
    category: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product content assessment SQL query failed")]
struct ProductListingContentAssessmentQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product content assessment row is invalid")]
struct ProductListingContentAssessmentMappingError {
    #[source]
    source: application::error::BoxError,
}

impl SqlxProductListingContentAssessmentReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductListingContentAssessmentReader for SqlxProductListingContentAssessmentReader {
    async fn find_current_assessments(
        &self,
        product_listing_ids: &[ProductListingId],
    ) -> Result<
        HashMap<ProductListingId, ProductListingContentAssessment>,
        ProductListingContentAssessmentReadError,
    > {
        if product_listing_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = sqlx::query_as::<_, ProductListingContentAssessmentRow>(
            r#"
            SELECT
                assessment.product_listing_id,
                assessment.source_event_id,
                assessment.decision,
                assessment.category
            FROM product_listings product
            JOIN product_listing_content_assessments assessment
              ON assessment.product_listing_id = product.product_listing_id
             AND assessment.source_event_id = product.content_source_event_id
            WHERE product.product_listing_id = ANY($1::uuid[])
            "#,
        )
        .bind(
            product_listing_ids
                .iter()
                .copied()
                .map(|id| id.into_uuid())
                .collect::<Vec<_>>(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(
            |source| ProductListingContentAssessmentReadError::QueryFailed {
                source: box_error(ProductListingContentAssessmentQueryError(source)),
            },
        )?;

        rows.into_iter()
            .map(|row| {
                let assessment = ProductListingContentAssessment::try_from(row)?;
                Ok((assessment.product_listing_id, assessment))
            })
            .collect()
    }
}

impl TryFrom<ProductListingContentAssessmentRow> for ProductListingContentAssessment {
    type Error = ProductListingContentAssessmentReadError;

    fn try_from(row: ProductListingContentAssessmentRow) -> Result<Self, Self::Error> {
        Ok(Self {
            product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")
                .map_err(mapping_error_source)?,
            source_event_id: try_from_uuid(row.source_event_id, "content source event ID")
                .map_err(mapping_error_source)?,
            decision: decode_decision(&row.decision, row.category.as_deref())?,
        })
    }
}

pub(crate) fn decode_decision(
    decision: &str,
    category: Option<&str>,
) -> Result<ContentPolicyDecision, ProductListingContentAssessmentReadError> {
    match (decision, category) {
        ("ALLOWED", None) => Ok(ContentPolicyDecision::Allowed),
        ("REQUIRES_CONSENT", Some(category)) => SensitiveContentCategory::from_code(category)
            .map(ContentPolicyDecision::RequiresConsent)
            .ok_or_else(|| {
                mapping_error("persisted product content assessment category is invalid")
            }),
        _ => Err(mapping_error(
            "persisted product content assessment decision and category are invalid",
        )),
    }
}

fn mapping_error(message: &'static str) -> ProductListingContentAssessmentReadError {
    ProductListingContentAssessmentReadError::InvalidPersistedState {
        source: box_error(ProductListingContentAssessmentMappingError {
            source: static_error(message),
        }),
    }
}

fn mapping_error_source(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductListingContentAssessmentReadError {
    ProductListingContentAssessmentReadError::InvalidPersistedState {
        source: box_error(ProductListingContentAssessmentMappingError {
            source: box_error(source),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_decode_exact_content_assessment_codecs() {
        assert!(matches!(
            decode_decision("ALLOWED", None),
            Ok(ContentPolicyDecision::Allowed)
        ));
        assert!(matches!(
            decode_decision("REQUIRES_CONSENT", Some("NAZI_GERMANY")),
            Ok(ContentPolicyDecision::RequiresConsent(
                SensitiveContentCategory::NaziGermany
            ))
        ));
    }

    #[test]
    fn should_reject_incomplete_content_assessment_codecs() {
        assert!(decode_decision("ALLOWED", Some("NAZI_GERMANY")).is_err());
        assert!(decode_decision("REQUIRES_CONSENT", None).is_err());
        assert!(decode_decision("allowed", None).is_err());
    }
}

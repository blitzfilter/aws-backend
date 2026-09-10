use crate::{object_id::try_from_uuid, url::referral_configuration};
use application::error::box_error;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use product_listing_service::ports::{
    ListingSourceSummary, ListingSourceSummaryReadError, ListingSourceSummaryReader,
    ListingSourceSummaryWithReferral,
};
use sqlx::PgPool;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct SqlxListingSourceSummaryReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ListingSourceSummaryRow {
    listing_source_id: uuid::Uuid,
    name: String,
    listing_source_slug_id: String,
    referral_configuration: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
#[error("listing source summary SQL query failed")]
struct ListingSourceSummaryQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("listing source summary row is invalid")]
struct ListingSourceSummaryMappingError {
    #[source]
    source: application::error::BoxError,
}

impl SqlxListingSourceSummaryReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ListingSourceSummaryReader for SqlxListingSourceSummaryReader {
    async fn find_summaries(
        &self,
        listing_source_ids: &[ListingSourceId],
    ) -> Result<
        HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
        ListingSourceSummaryReadError,
    > {
        if listing_source_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let rows = sqlx::query_as::<_, ListingSourceSummaryRow>(
            r#"
            SELECT listing_source_id, name, listing_source_slug_id, referral_configuration
            FROM listing_sources
            WHERE listing_source_id = ANY($1::uuid[])
            "#,
        )
        .bind(
            listing_source_ids
                .iter()
                .copied()
                .map(|id| id.into_uuid())
                .collect::<Vec<_>>(),
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| ListingSourceSummaryReadError::QueryFailed {
            source: box_error(ListingSourceSummaryQueryError(source)),
        })?;

        rows.into_iter()
            .map(|row| {
                let summary = ListingSourceSummaryWithReferral::try_from(row)?;
                Ok((summary.summary.listing_source_id, summary))
            })
            .collect()
    }
}

impl TryFrom<ListingSourceSummaryRow> for ListingSourceSummaryWithReferral {
    type Error = ListingSourceSummaryReadError;

    fn try_from(row: ListingSourceSummaryRow) -> Result<Self, Self::Error> {
        Ok(Self {
            summary: ListingSourceSummary {
                listing_source_id: try_from_uuid(row.listing_source_id, "ListingSource ID")
                    .map_err(|source| ListingSourceSummaryReadError::InvalidReadModel {
                        source: box_error(ListingSourceSummaryMappingError {
                            source: box_error(source),
                        }),
                    })?,
                name: ListingSourceName::try_from(row.name).map_err(|source| {
                    ListingSourceSummaryReadError::InvalidReadModel {
                        source: box_error(ListingSourceSummaryMappingError {
                            source: box_error(source),
                        }),
                    }
                })?,
                slug_id: ListingSourceSlugId::raw(row.listing_source_slug_id).map_err(
                    |source| ListingSourceSummaryReadError::InvalidReadModel {
                        source: box_error(ListingSourceSummaryMappingError {
                            source: box_error(source),
                        }),
                    },
                )?,
            },
            referral_configuration: referral_configuration(row.referral_configuration.as_ref())
                .map_err(|_| ListingSourceSummaryReadError::InvalidReadModel {
                    source: box_error(ListingSourceSummaryMappingError {
                        source: application::error::static_error(
                            "persisted listing source referral configuration is invalid",
                        ),
                    }),
                })?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_invalid_persisted_listing_source_name_and_slug() {
        let error = ListingSourceSummaryWithReferral::try_from(ListingSourceSummaryRow {
            listing_source_id: ListingSourceId::new().into_uuid(),
            name: "\u{2003}".to_owned(),
            listing_source_slug_id: "".to_owned(),
            referral_configuration: None,
        });

        assert!(matches!(
            error,
            Err(ListingSourceSummaryReadError::InvalidReadModel { .. })
        ));
    }

    #[test]
    fn should_preserve_listing_source_summary_query_source() {
        let error = ListingSourceSummaryReadError::QueryFailed {
            source: box_error(ListingSourceSummaryQueryError(sqlx::Error::RowNotFound)),
        };

        let ListingSourceSummaryReadError::QueryFailed { source } = error else {
            panic!("expected listing source summary query failure");
        };
        assert!(
            source
                .downcast_ref::<ListingSourceSummaryQueryError>()
                .is_some()
        );
        assert!(source.source().is_some());
    }

    #[test]
    fn should_preserve_listing_source_summary_mapping_source() {
        let error = ListingSourceSummaryReadError::InvalidReadModel {
            source: box_error(ListingSourceSummaryMappingError {
                source: application::error::static_error("invalid listing source slug"),
            }),
        };

        let ListingSourceSummaryReadError::InvalidReadModel { source } = error else {
            panic!("expected listing source summary mapping failure");
        };
        assert!(
            source
                .downcast_ref::<ListingSourceSummaryMappingError>()
                .is_some()
        );
        assert!(source.source().is_some());
    }
}

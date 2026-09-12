use crate::object_id::try_from_uuid;
use application::error::box_error;

use product_listing_core::product_listing_event::{
    ProductListingEventPayload, ProductListingLifecycleChange,
};
use product_listing_service::{
    ports::{
        ProductListingHistoryReadError, ProductListingHistoryReader,
        ProductListingHistoryReaderFactory,
    },
    use_cases::{
        ProductListingDiscoveryHistory, ProductListingHistoryChange, ProductListingHistoryChanges,
        ProductListingHistoryEntry, ProductListingHistoryEntryKind, ProductListingHistoryLookup,
    },
};
use serde_json::Value;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::product_listing_event_codec;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingHistoryReaderFactory;

struct SqlxProductListingHistoryReader<'tx> {
    connection: &'tx mut PgConnection,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingHistoryRow {
    product_listing_id: uuid::Uuid,
    event_id: uuid::Uuid,
    event_type: String,
    event_group: String,
    event_type_schema_version: i16,
    payload: Value,
    event_time: OffsetDateTime,
}

impl SqlxProductListingHistoryReaderFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingHistoryReaderFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingHistoryReaderFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingHistoryReader + 'tx {
        SqlxProductListingHistoryReader {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingHistoryReader for SqlxProductListingHistoryReader<'_> {
    async fn find_history(
        &mut self,
        lookup: &ProductListingHistoryLookup,
    ) -> Result<Option<Vec<ProductListingHistoryEntry>>, ProductListingHistoryReadError> {
        let product_listing_id = match lookup {
            ProductListingHistoryLookup::ById(product_listing_id) => {
                sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT product_listing_id FROM product_listings WHERE product_listing_id = $1",
                )
                .bind(product_listing_id.as_uuid())
                .fetch_optional(&mut *self.connection)
                .await
            }
            ProductListingHistoryLookup::ByTitleSlug(product_listing_title_slug_id) => {
                sqlx::query_scalar::<_, uuid::Uuid>(
                    "SELECT product_listing_id FROM product_listings WHERE product_listing_title_slug_id = $1",
                )
                .bind(product_listing_title_slug_id.as_ref())
                .fetch_optional(&mut *self.connection)
                .await
            }
        }
        .map_err(|error| ProductListingHistoryReadError::ProductListingHistoryQueryFailed {
            source: box_error(ProductListingHistoryQuerySqlxError(error)),
        })?;

        let Some(product_listing_id) = product_listing_id else {
            return Ok(None);
        };
        let product_listing_id = try_from_uuid::<
            product_listing_core::product_listing_id::ProductListingId,
        >(product_listing_id, "ProductListing ID")
        .map_err(|source| {
            ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid {
                source: box_error(source),
            }
        })?;

        let rows = sqlx::query_as::<_, ProductListingHistoryRow>(
            r#"
            SELECT
                event_id,
                product_listing_id,
                event_type,
                event_group,
                event_type_schema_version,
                payload,
                event_time
            FROM product_listing_events
            WHERE product_listing_id = $1
              AND event_group = 'DOMAIN'
              AND event_type IN (
                  'PRODUCT_LISTING_DISCOVERED',
                  'PRODUCT_LISTING_CHANGED'
              )
            ORDER BY event_time ASC, event_id ASC
            "#,
        )
        .bind(product_listing_id.as_uuid())
        .fetch_all(&mut *self.connection)
        .await
        .map_err(|error| {
            ProductListingHistoryReadError::ProductListingHistoryQueryFailed {
                source: box_error(ProductListingHistoryQuerySqlxError(error)),
            }
        })?;

        rows.into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("product listing history SQL query failed")]
struct ProductListingHistoryQuerySqlxError(#[source] sqlx::Error);

impl TryFrom<ProductListingHistoryRow> for ProductListingHistoryEntry {
    type Error = ProductListingHistoryReadError;

    fn try_from(row: ProductListingHistoryRow) -> Result<Self, Self::Error> {
        let payload = match product_listing_event_codec::decode_persisted(
            &row.event_type,
            &row.event_group,
            row.event_type_schema_version,
            &row.payload,
        )
        .map_err(|error| {
            ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid {
                source: box_error(error),
            }
        })? {
            product_listing_event_codec::ProductListingPersistedEvent::Domain(_, payload) => {
                *payload
            }
            _ => {
                return Err(
                    ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid {
                        source: box_error(std::io::Error::other(
                            "non-domain event in ProductListing history",
                        )),
                    },
                );
            }
        };

        let kind = history_kind(payload)?;

        Ok(Self {
            product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")
                .map_err(|source| {
                    ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid {
                        source: box_error(source),
                    }
                })?,
            event_id: try_from_uuid(row.event_id, "event ID").map_err(|source| {
                ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid {
                    source: box_error(source),
                }
            })?,
            occurred_at: row.event_time,
            kind,
        })
    }
}

fn history_kind(
    payload: ProductListingEventPayload,
) -> Result<ProductListingHistoryEntryKind, ProductListingHistoryReadError> {
    match payload {
        ProductListingEventPayload::Discovered(discovered) => Ok(
            ProductListingHistoryEntryKind::Discovered(Box::new(ProductListingDiscoveryHistory {
                listing_source_id: discovered.listing_source_id(),
                source_listing_id: discovered.source_listing_id().clone(),
                title: discovered.title().cloned(),
                description: discovered.description().cloned(),
                pricing: discovered.pricing(),
                availability: discovered.availability(),
                url: discovered.url().clone(),
                image_count: discovered.image_count().value(),
                auction: discovered.auction().cloned(),
            })),
        ),
        ProductListingEventPayload::Changed(changed) => {
            let mut changes = Vec::new();

            if let Some(change) = changed.price() {
                changes.push(ProductListingHistoryChange::MainPriceChanged {
                    previous: *change.previous(),
                    current: *change.current(),
                });
            }
            if let Some(change) = changed.price_estimate_min() {
                changes.push(ProductListingHistoryChange::MinimumEstimateChanged {
                    previous: *change.previous(),
                    current: *change.current(),
                });
            }
            if let Some(change) = changed.price_estimate_max() {
                changes.push(ProductListingHistoryChange::MaximumEstimateChanged {
                    previous: *change.previous(),
                    current: *change.current(),
                });
            }
            if let Some(change) = changed.availability() {
                changes.push(ProductListingHistoryChange::AvailabilityChanged {
                    previous: *change.previous(),
                    current: *change.current(),
                });
            }
            if let Some(change) = changed.url() {
                changes.push(ProductListingHistoryChange::UrlChanged {
                    previous: change.previous().clone(),
                    current: change.current().clone(),
                });
            }
            if let Some(change) = changed.image_count() {
                changes.push(ProductListingHistoryChange::ImagesChanged {
                    previous_count: change.previous_count().value(),
                    current_count: change.current_count().value(),
                });
            }
            if let Some(change) = changed.auction() {
                changes.push(ProductListingHistoryChange::AuctionChanged {
                    previous: change.previous().clone(),
                    current: change.current().clone(),
                });
            }
            if let Some(change) = changed.lifecycle() {
                changes.push(match change {
                    ProductListingLifecycleChange::Withdrawn {
                        previous_availability,
                    } => ProductListingHistoryChange::Withdrawn {
                        previous_availability: *previous_availability,
                    },
                    ProductListingLifecycleChange::Restored => {
                        ProductListingHistoryChange::Restored
                    }
                });
            }
            if let Some(change) = changed.sale_observation() {
                let history_change = match (change.previous(), change.current()) {
                    (None, Some(observation)) => ProductListingHistoryChange::SaleObserved {
                        observation: *observation,
                    },
                    (Some(observation), None) => {
                        ProductListingHistoryChange::SaleObservationRetracted {
                            observation: *observation,
                        }
                    }
                    _ => {
                        return Err(
                            ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid {
                                source: box_error(std::io::Error::other(
                                    "invalid sale observation transition in ProductListing history",
                                )),
                            },
                        );
                    }
                };
                changes.push(history_change);
            }

            ProductListingHistoryChanges::try_from(changes)
                .map(ProductListingHistoryEntryKind::Changed)
                .map_err(|error| {
                    ProductListingHistoryReadError::ProductListingHistoryReadModelInvalid {
                        source: box_error(error),
                    }
                })
        }
    }
}

use crate::scraper::css_selector::removed_page_schema::{
    ListingSourceRemovedPageSchema, RemovedPageSchema,
};
use listing_source_core::ListingSourceId;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

#[async_trait::async_trait]
#[mockall::automock]
pub trait RemovedPageSchemaRepository {
    async fn find_removed_page_schema(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<ListingSourceRemovedPageSchema>, sqlx::Error>;

    async fn insert_removed_page_schema(
        &self,
        listing_source_id: &ListingSourceId,
        schema: &ListingSourceRemovedPageSchema,
    ) -> Result<ListingSourceRemovedPageSchema, sqlx::Error>;

    async fn update_removed_page_schema(
        &self,
        listing_source_id: &ListingSourceId,
        removed_page_schemas: &[RemovedPageSchema],
    ) -> Result<ListingSourceRemovedPageSchema, sqlx::Error>;
}

/// Disabled persistence fallback used by tests and simple constructors.
///
/// Reads act as an empty schema cache. Writes fail because this repository does not own durable
/// storage; production wires [`RemovedPageSchemaRepositoryImpl`] explicitly.
pub struct NullRemovedPageSchemaRepository;

#[async_trait::async_trait]
impl RemovedPageSchemaRepository for NullRemovedPageSchemaRepository {
    async fn find_removed_page_schema(
        &self,
        _: &ListingSourceId,
    ) -> Result<Option<ListingSourceRemovedPageSchema>, sqlx::Error> {
        Ok(None)
    }

    async fn insert_removed_page_schema(
        &self,
        _: &ListingSourceId,
        _: &ListingSourceRemovedPageSchema,
    ) -> Result<ListingSourceRemovedPageSchema, sqlx::Error> {
        Err(sqlx::Error::RowNotFound)
    }

    async fn update_removed_page_schema(
        &self,
        _: &ListingSourceId,
        _: &[RemovedPageSchema],
    ) -> Result<ListingSourceRemovedPageSchema, sqlx::Error> {
        Err(sqlx::Error::RowNotFound)
    }
}

pub struct RemovedPageSchemaRepositoryImpl<'a> {
    pool: &'a PgPool,
}

impl<'a> RemovedPageSchemaRepositoryImpl<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_schema(
    row: sqlx::postgres::PgRow,
) -> Result<ListingSourceRemovedPageSchema, sqlx::Error> {
    let listing_source_id_uuid: Uuid = row.try_get("listing_source_id")?;
    let schema_json: serde_json::Value = row.try_get("removed_page_schema")?;
    let created: OffsetDateTime = row.try_get("created")?;
    let updated: OffsetDateTime = row.try_get("updated")?;

    let removed_page_schemas: Vec<RemovedPageSchema> =
        serde_json::from_value(schema_json).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
    Ok(ListingSourceRemovedPageSchema {
        listing_source_id: ListingSourceId::try_from(listing_source_id_uuid)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        removed_page_schemas,
        created,
        updated,
    })
}

#[async_trait::async_trait]
impl<'a> RemovedPageSchemaRepository for RemovedPageSchemaRepositoryImpl<'a> {
    async fn find_removed_page_schema(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<ListingSourceRemovedPageSchema>, sqlx::Error> {
        sqlx::query(
            "SELECT listing_source_id, removed_page_schema, created, updated
             FROM listing_source_removed_page_schemas
             WHERE listing_source_id = $1",
        )
        .bind(Uuid::from(*listing_source_id))
        .fetch_optional(self.pool)
        .await?
        .map(row_to_schema)
        .transpose()
    }

    async fn insert_removed_page_schema(
        &self,
        listing_source_id: &ListingSourceId,
        schema: &ListingSourceRemovedPageSchema,
    ) -> Result<ListingSourceRemovedPageSchema, sqlx::Error> {
        let schema_json = serde_json::to_value(&schema.removed_page_schemas)
            .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;

        sqlx::query(
            "INSERT INTO listing_source_removed_page_schemas (listing_source_id, removed_page_schema, created, updated)
             VALUES ($1, $2, $3, $4)
             RETURNING listing_source_id, removed_page_schema, created, updated",
        )
        .bind(Uuid::from(*listing_source_id))
        .bind(schema_json)
        .bind(schema.created)
        .bind(schema.updated)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_schema)
    }

    async fn update_removed_page_schema(
        &self,
        listing_source_id: &ListingSourceId,
        removed_page_schemas: &[RemovedPageSchema],
    ) -> Result<ListingSourceRemovedPageSchema, sqlx::Error> {
        let schema_json = serde_json::to_value(removed_page_schemas)
            .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;

        sqlx::query(
            "UPDATE listing_source_removed_page_schemas
             SET removed_page_schema = $2, updated = NOW()
             WHERE listing_source_id = $1
             RETURNING listing_source_id, removed_page_schema, created, updated",
        )
        .bind(Uuid::from(*listing_source_id))
        .bind(schema_json)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_schema)
    }
}

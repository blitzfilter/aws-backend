use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use listing_source_core::ListingSourceId;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

#[async_trait::async_trait]
#[mockall::automock]
pub trait ListingSourceProductSchemaRepository {
    async fn find_product_schema(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<ListingSourceProductSchema>, sqlx::Error>;

    async fn insert_product_schema(
        &self,
        listing_source_id: &ListingSourceId,
        schema: &ListingSourceProductSchema,
    ) -> Result<ListingSourceProductSchema, sqlx::Error>;

    async fn update_product_schema(
        &self,
        listing_source_id: &ListingSourceId,
        product_schemas: &[ProductCssSelectorSchema],
    ) -> Result<ListingSourceProductSchema, sqlx::Error>;
}

pub struct ListingSourceProductSchemaRepositoryImpl<'a> {
    pool: &'a PgPool,
}

impl<'a> ListingSourceProductSchemaRepositoryImpl<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }
}

fn row_to_schema(row: sqlx::postgres::PgRow) -> Result<ListingSourceProductSchema, sqlx::Error> {
    let listing_source_id_uuid: Uuid = row.try_get("listing_source_id")?;
    let product_schema_json: serde_json::Value = row.try_get("product_schema")?;
    let created: OffsetDateTime = row.try_get("created")?;
    let updated: OffsetDateTime = row.try_get("updated")?;

    let product_schemas: Vec<ProductCssSelectorSchema> = match serde_json::from_value::<
        Vec<ProductCssSelectorSchema>,
    >(product_schema_json.clone())
    {
        Ok(list) => list,
        Err(_) => vec![
            serde_json::from_value::<ProductCssSelectorSchema>(product_schema_json)
                .map_err(|e| sqlx::Error::Decode(Box::new(e)))?,
        ],
    };
    Ok(ListingSourceProductSchema {
        listing_source_id: ListingSourceId::try_from(listing_source_id_uuid)
            .map_err(|error| sqlx::Error::Decode(Box::new(error)))?,
        product_schemas,
        created,
        updated,
    })
}

#[async_trait::async_trait]
impl<'a> ListingSourceProductSchemaRepository for ListingSourceProductSchemaRepositoryImpl<'a> {
    async fn find_product_schema(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Option<ListingSourceProductSchema>, sqlx::Error> {
        sqlx::query(
            "SELECT listing_source_id, product_schema, created, updated
             FROM listing_source_product_schemas
             WHERE listing_source_id = $1",
        )
        .bind(Uuid::from(*listing_source_id))
        .fetch_optional(self.pool)
        .await?
        .map(row_to_schema)
        .transpose()
    }

    async fn insert_product_schema(
        &self,
        listing_source_id: &ListingSourceId,
        schema: &ListingSourceProductSchema,
    ) -> Result<ListingSourceProductSchema, sqlx::Error> {
        let schemas = schema.product_schemas.clone();
        let product_schema_json =
            serde_json::to_value(&schemas).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;

        sqlx::query(
            "INSERT INTO listing_source_product_schemas (listing_source_id, product_schema, created, updated)
             VALUES ($1, $2, $3, $4)
             RETURNING listing_source_id, product_schema, created, updated",
        )
        .bind(Uuid::from(*listing_source_id))
        .bind(product_schema_json)
        .bind(schema.created)
        .bind(schema.updated)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_schema)
    }

    async fn update_product_schema(
        &self,
        listing_source_id: &ListingSourceId,
        product_schemas: &[ProductCssSelectorSchema],
    ) -> Result<ListingSourceProductSchema, sqlx::Error> {
        let product_schema_json =
            serde_json::to_value(product_schemas).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;

        sqlx::query(
            "UPDATE listing_source_product_schemas
             SET product_schema = $2, updated = NOW()
             WHERE listing_source_id = $1
             RETURNING listing_source_id, product_schema, created, updated",
        )
        .bind(Uuid::from(*listing_source_id))
        .bind(product_schema_json)
        .fetch_one(self.pool)
        .await
        .and_then(row_to_schema)
    }
}

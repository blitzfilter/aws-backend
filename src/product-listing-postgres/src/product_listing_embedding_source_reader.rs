use crate::object_id::try_from_uuid;
use application::error::{box_error, static_error};
use domain_primitives::event_id::EventId;
use localization::Language;
use localization::Localized;
use product_listing_core::{
    description::Description, product_listing_event::ProductListingEventPayload,
    product_listing_id::ProductListingId, title::Title,
};
use product_listing_service::ports::{
    ProductListingEmbeddingSource, ProductListingEmbeddingSourceEvent,
    ProductListingEmbeddingSourceReadError, ProductListingEmbeddingSourceReader,
};
use serde::Deserialize;
use sqlx::PgPool;
use url::Url;

use crate::product_listing_event_codec;

#[derive(Clone)]
pub struct SqlxProductListingEmbeddingSourceReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingEmbeddingSourceRow {
    product_listing_id: uuid::Uuid,
    event_id: uuid::Uuid,
    embedding_source_event_id: uuid::Uuid,
    event_type: String,
    event_group: String,
    event_type_schema_version: i16,
    payload: serde_json::Value,
    title_text: Option<String>,
    title_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
    product_images: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ProductListingImageRecord {
    url: String,
}

#[derive(Debug, thiserror::Error)]
#[error("product embedding source SQL query failed")]
struct ProductListingEmbeddingSourceQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product embedding source row is invalid")]
struct ProductListingEmbeddingSourceMappingError {
    #[source]
    source: application::error::BoxError,
}

impl SqlxProductListingEmbeddingSourceReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductListingEmbeddingSourceReader for SqlxProductListingEmbeddingSourceReader {
    async fn find_source(
        &self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingEmbeddingSource>, ProductListingEmbeddingSourceReadError> {
        let row = sqlx::query_as::<_, ProductListingEmbeddingSourceRow>(
            r#"
            SELECT event.event_id, event.product_listing_id, event.event_type,
                   event.event_group, event.event_type_schema_version, event.payload,
                   product.embedding_source_event_id,
                   product.title_text, product.title_language,
                   product.description_text, product.description_language,
                   product.product_images
            FROM product_listing_events event
            JOIN product_listings product ON product.product_listing_id = event.product_listing_id
            WHERE event.event_id = $1 AND event.product_listing_id = $2
        "#,
        )
        .bind(event_id.as_uuid())
        .bind(product_listing_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(
            |source| ProductListingEmbeddingSourceReadError::QueryFailed {
                source: box_error(ProductListingEmbeddingSourceQueryError(source)),
            },
        )?;
        row.map(TryInto::try_into).transpose()
    }
}

impl TryFrom<ProductListingEmbeddingSourceRow> for ProductListingEmbeddingSource {
    type Error = ProductListingEmbeddingSourceReadError;

    fn try_from(row: ProductListingEmbeddingSourceRow) -> Result<Self, Self::Error> {
        Ok(Self {
            product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")
                .map_err(mapping_error_source)?,
            event_id: try_from_uuid(row.event_id, "event ID").map_err(mapping_error_source)?,
            embedding_source_event_id: try_from_uuid(
                row.embedding_source_event_id,
                "embedding source event ID",
            )
            .map_err(mapping_error_source)?,
            event: embedding_event(&row)?,
            title: localized_title(row.title_text, row.title_language)?,
            description: localized_description(row.description_text, row.description_language)?,
            image_url: first_image_url(row.product_images)?,
        })
    }
}

fn embedding_event(
    row: &ProductListingEmbeddingSourceRow,
) -> Result<ProductListingEmbeddingSourceEvent, ProductListingEmbeddingSourceReadError> {
    let event = product_listing_event_codec::decode_persisted(
        &row.event_type,
        &row.event_group,
        row.event_type_schema_version,
        &row.payload,
    )
    .map_err(
        |source| ProductListingEmbeddingSourceReadError::InvalidPersistedState {
            source: box_error(source),
        },
    )?;
    Ok(match event {
        product_listing_event_codec::ProductListingPersistedEvent::Domain(_, payload) => {
            match *payload {
                ProductListingEventPayload::Discovered(_) => {
                    ProductListingEmbeddingSourceEvent::Discovered
                }
                ProductListingEventPayload::Changed(changed) if changed.image_count().is_some() => {
                    ProductListingEmbeddingSourceEvent::ChangedImages
                }
                ProductListingEventPayload::Changed(_) => ProductListingEmbeddingSourceEvent::Other,
            }
        }
        product_listing_event_codec::ProductListingPersistedEvent::Embedded
        | product_listing_event_codec::ProductListingPersistedEvent::TranslatedTitles => {
            ProductListingEmbeddingSourceEvent::Other
        }
    })
}

fn localized_title(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Title>>, ProductListingEmbeddingSourceReadError> {
    match (text, language) {
        (Some(raw_text), Some(raw_language)) => {
            let title = Title::from(raw_text.as_str());
            if title.as_ref().is_empty() || title.as_ref() != raw_text {
                return Err(mapping_error(
                    "persisted product embedding title is invalid",
                ));
            }
            Ok(Some(Localized::new(parse_language(&raw_language)?, title)))
        }
        (None, None) => Ok(None),
        _ => Err(mapping_error(
            "persisted product embedding title is incomplete",
        )),
    }
}

fn localized_description(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, Description>>, ProductListingEmbeddingSourceReadError> {
    match (text, language) {
        (Some(raw_text), Some(raw_language)) => {
            let description = Description::from(raw_text.as_str());
            if description.as_ref().is_empty() || description.as_ref() != raw_text {
                return Err(mapping_error(
                    "persisted product embedding description is invalid",
                ));
            }
            Ok(Some(Localized::new(
                parse_language(&raw_language)?,
                description,
            )))
        }
        (None, None) => Ok(None),
        _ => Err(mapping_error(
            "persisted product embedding description is incomplete",
        )),
    }
}

fn first_image_url(
    value: serde_json::Value,
) -> Result<Option<Url>, ProductListingEmbeddingSourceReadError> {
    let images: Vec<ProductListingImageRecord> = serde_json::from_value(value)
        .map_err(|_| mapping_error("persisted product embedding images are invalid"))?;
    images
        .into_iter()
        .next()
        .map(|image| {
            let url = Url::parse(&image.url)
                .map_err(|_| mapping_error("persisted product embedding image URL is invalid"))?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(mapping_error(
                    "persisted product embedding image URL is invalid",
                ));
            }
            Ok(url)
        })
        .transpose()
}

fn parse_language(value: &str) -> Result<Language, ProductListingEmbeddingSourceReadError> {
    Language::from_code(value)
        .ok_or_else(|| mapping_error("persisted product embedding language is invalid"))
}

fn mapping_error(message: &'static str) -> ProductListingEmbeddingSourceReadError {
    ProductListingEmbeddingSourceReadError::InvalidPersistedState {
        source: box_error(ProductListingEmbeddingSourceMappingError {
            source: static_error(message),
        }),
    }
}

fn mapping_error_source(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductListingEmbeddingSourceReadError {
    ProductListingEmbeddingSourceReadError::InvalidPersistedState {
        source: box_error(ProductListingEmbeddingSourceMappingError {
            source: box_error(source),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_noncanonical_embedding_source_language() {
        assert!(parse_language("EN").is_err());
    }
}

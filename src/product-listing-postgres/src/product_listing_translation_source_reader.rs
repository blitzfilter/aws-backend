use crate::object_id::try_from_uuid;
use application::error::{box_error, static_error};
use domain_primitives::event_id::EventId;
use localization::Language;
use product_listing_core::{
    product_listing_event::ProductListingEventPayload, product_listing_id::ProductListingId,
    title::Title,
};
use product_listing_service::ports::{
    ProductListingTranslationSource, ProductListingTranslationSourceEvent,
    ProductListingTranslationSourceReadError, ProductListingTranslationSourceReader,
};
use sqlx::PgPool;

use crate::product_listing_event_codec;

#[derive(Clone)]
pub struct SqlxProductListingTranslationSourceReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingTranslationSourceRow {
    product_listing_id: uuid::Uuid,
    event_id: uuid::Uuid,
    content_source_event_id: uuid::Uuid,
    event_type: String,
    event_group: String,
    event_type_schema_version: i16,
    payload: serde_json::Value,
    title_text: Option<String>,
    title_language: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product translation source SQL query failed")]
struct ProductListingTranslationSourceQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product translation source row is invalid")]
struct ProductListingTranslationSourceMappingError {
    #[source]
    source: application::error::BoxError,
}

impl From<ProductListingTranslationSourceQueryError> for ProductListingTranslationSourceReadError {
    fn from(source: ProductListingTranslationSourceQueryError) -> Self {
        Self::QueryFailed {
            source: box_error(source),
        }
    }
}

impl SqlxProductListingTranslationSourceReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductListingTranslationSourceReader for SqlxProductListingTranslationSourceReader {
    async fn find_source(
        &self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListingTranslationSource>, ProductListingTranslationSourceReadError>
    {
        let row = sqlx::query_as::<_, ProductListingTranslationSourceRow>(
            r#"
            SELECT
                event.event_id,
                event.product_listing_id,
                event.event_type,
                event.event_group,
                event.event_type_schema_version,
                event.payload,
                product.content_source_event_id,
                product.title_text,
                product.title_language
            FROM product_listing_events event
            JOIN product_listings product ON product.product_listing_id = event.product_listing_id
            WHERE event.event_id = $1
              AND event.product_listing_id = $2
            "#,
        )
        .bind(event_id.as_uuid())
        .bind(product_listing_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(ProductListingTranslationSourceQueryError)?;

        row.map(TryInto::try_into).transpose()
    }
}

impl TryFrom<ProductListingTranslationSourceRow> for ProductListingTranslationSource {
    type Error = ProductListingTranslationSourceReadError;

    fn try_from(row: ProductListingTranslationSourceRow) -> Result<Self, Self::Error> {
        let event = translation_event(&row)?;
        let (title, title_language) = match (row.title_text, row.title_language) {
            (Some(raw_title), Some(raw_language)) => {
                let title = Title::from(raw_title.as_str());
                if title.as_ref().is_empty() || title.as_ref() != raw_title {
                    return Err(mapping_error(
                        "persisted product translation title is invalid",
                    ));
                }
                (Some(title), Some(parse_language(&raw_language)?))
            }
            (None, None) => (None, None),
            _ => {
                return Err(mapping_error(
                    "persisted product translation title is incomplete",
                ));
            }
        };

        Ok(Self {
            product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")
                .map_err(mapping_error_source)?,
            event_id: try_from_uuid(row.event_id, "event ID").map_err(mapping_error_source)?,
            content_source_event_id: try_from_uuid(
                row.content_source_event_id,
                "content source event ID",
            )
            .map_err(mapping_error_source)?,
            event,
            title,
            title_language,
        })
    }
}

fn translation_event(
    row: &ProductListingTranslationSourceRow,
) -> Result<ProductListingTranslationSourceEvent, ProductListingTranslationSourceReadError> {
    let event = product_listing_event_codec::decode_persisted(
        &row.event_type,
        &row.event_group,
        row.event_type_schema_version,
        &row.payload,
    )
    .map_err(
        |source| ProductListingTranslationSourceReadError::InvalidPersistedState {
            source: box_error(source),
        },
    )?;
    Ok(match event {
        product_listing_event_codec::ProductListingPersistedEvent::Domain(_, payload) => {
            match *payload {
                ProductListingEventPayload::Discovered(_) => {
                    ProductListingTranslationSourceEvent::Discovered
                }
                ProductListingEventPayload::Changed(_) => {
                    ProductListingTranslationSourceEvent::Other
                }
            }
        }
        _ => ProductListingTranslationSourceEvent::Other,
    })
}

fn parse_language(value: &str) -> Result<Language, ProductListingTranslationSourceReadError> {
    Language::from_code(value)
        .ok_or_else(|| mapping_error("persisted product translation language is invalid"))
}

fn mapping_error(message: &'static str) -> ProductListingTranslationSourceReadError {
    ProductListingTranslationSourceReadError::InvalidPersistedState {
        source: box_error(ProductListingTranslationSourceMappingError {
            source: static_error(message),
        }),
    }
}

fn mapping_error_source(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductListingTranslationSourceReadError {
    ProductListingTranslationSourceReadError::InvalidPersistedState {
        source: box_error(ProductListingTranslationSourceMappingError {
            source: box_error(source),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_reject_noncanonical_translation_source_language() {
        assert!(parse_language("EN").is_err());
    }
}

use crate::object_id::try_from_uuid;
use application::error::{box_error, static_error};
use domain_primitives::event_id::EventId;
use localization::Language;
use product_listing_core::{
    description::Description, product_listing_id::ProductListingId, title::Title,
};
use product_listing_service::ports::{
    ProductListingContentAssessmentSource, ProductListingContentAssessmentSourceEvent,
    ProductListingContentAssessmentSourceReadError, ProductListingContentAssessmentSourceReader,
};
use sqlx::PgPool;

use crate::product_listing_event_codec;

#[derive(Clone)]
pub struct SqlxProductListingContentAssessmentSourceReader {
    pool: PgPool,
}

#[derive(Debug, sqlx::FromRow)]
struct ProductListingContentAssessmentSourceRow {
    product_listing_id: uuid::Uuid,
    event_id: uuid::Uuid,
    current_content_source_event_id: uuid::Uuid,
    event_group: String,
    event_type: String,
    event_type_schema_version: i16,
    payload: serde_json::Value,
    title_text: Option<String>,
    title_language: Option<String>,
    description_text: Option<String>,
    description_language: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("product content assessment source SQL query failed")]
struct ProductListingContentAssessmentSourceQueryError(#[source] sqlx::Error);

#[derive(Debug, thiserror::Error)]
#[error("product content assessment source row is invalid")]
struct ProductListingContentAssessmentSourceMappingError {
    #[source]
    source: application::error::BoxError,
}

impl SqlxProductListingContentAssessmentSourceReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProductListingContentAssessmentSourceReader
    for SqlxProductListingContentAssessmentSourceReader
{
    async fn find_source(
        &self,
        event_id: EventId,
        product_listing_id: ProductListingId,
    ) -> Result<
        Option<ProductListingContentAssessmentSource>,
        ProductListingContentAssessmentSourceReadError,
    > {
        let row = sqlx::query_as::<_, ProductListingContentAssessmentSourceRow>(
            r#"
            SELECT
                event.product_listing_id,
                event.event_id,
                product.content_source_event_id AS current_content_source_event_id,
                event.event_group,
                event.event_type,
                event.event_type_schema_version,
                event.payload,
                product.title_text,
                product.title_language,
                product.description_text,
                product.description_language
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
        .map_err(
            |source| ProductListingContentAssessmentSourceReadError::QueryFailed {
                source: box_error(ProductListingContentAssessmentSourceQueryError(source)),
            },
        )?;

        row.map(TryInto::try_into).transpose()
    }
}

impl TryFrom<ProductListingContentAssessmentSourceRow> for ProductListingContentAssessmentSource {
    type Error = ProductListingContentAssessmentSourceReadError;

    fn try_from(row: ProductListingContentAssessmentSourceRow) -> Result<Self, Self::Error> {
        let event = content_assessment_event(&row)?;
        Ok(Self {
            product_listing_id: try_from_uuid(row.product_listing_id, "ProductListing ID")
                .map_err(mapping_error_source)?,
            event_id: try_from_uuid(row.event_id, "event ID").map_err(mapping_error_source)?,
            current_content_source_event_id: try_from_uuid(
                row.current_content_source_event_id,
                "content source event ID",
            )
            .map_err(mapping_error_source)?,
            event,
            title: content_title(row.title_text, row.title_language)?,
            description: content_description(row.description_text, row.description_language)?,
        })
    }
}

fn content_assessment_event(
    row: &ProductListingContentAssessmentSourceRow,
) -> Result<
    ProductListingContentAssessmentSourceEvent,
    ProductListingContentAssessmentSourceReadError,
> {
    let event = product_listing_event_codec::decode_persisted(
        &row.event_type,
        &row.event_group,
        row.event_type_schema_version,
        &row.payload,
    )
    .map_err(|source| {
        ProductListingContentAssessmentSourceReadError::InvalidPersistedState {
            source: box_error(source),
        }
    })?;
    Ok(match event {
        product_listing_event_codec::ProductListingPersistedEvent::Domain(_, payload) => {
            match *payload {
                product_listing_core::product_listing_event::ProductListingEventPayload::Discovered(_) => {
                    ProductListingContentAssessmentSourceEvent::Discovered
                }
                product_listing_core::product_listing_event::ProductListingEventPayload::Changed(_) => {
                    ProductListingContentAssessmentSourceEvent::Other
                }
            }
        }
        _ => ProductListingContentAssessmentSourceEvent::Other,
    })
}

fn content_title(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Title>, ProductListingContentAssessmentSourceReadError> {
    match (text, language) {
        (Some(text), Some(language)) => {
            parse_language(&language)?;
            let title = Title::from(text.as_str());
            if title.as_ref() != text {
                return Err(mapping_error(
                    "persisted product content assessment title is invalid",
                ));
            }
            Ok((!title.as_ref().is_empty()).then_some(title))
        }
        (None, None) => Ok(None),
        _ => Err(mapping_error(
            "persisted product content assessment title is incomplete",
        )),
    }
}

fn content_description(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Description>, ProductListingContentAssessmentSourceReadError> {
    match (text, language) {
        (Some(text), Some(language)) => {
            parse_language(&language)?;
            let description = Description::from(text.as_str());
            if description.as_ref() != text {
                return Err(mapping_error(
                    "persisted product content assessment description is invalid",
                ));
            }
            Ok((!description.as_ref().is_empty()).then_some(description))
        }
        (None, None) => Ok(None),
        _ => Err(mapping_error(
            "persisted product content assessment description is incomplete",
        )),
    }
}

fn parse_language(value: &str) -> Result<(), ProductListingContentAssessmentSourceReadError> {
    Language::from_code(value)
        .map(|_| ())
        .ok_or_else(|| mapping_error("persisted product content assessment language is invalid"))
}

fn mapping_error(message: &'static str) -> ProductListingContentAssessmentSourceReadError {
    ProductListingContentAssessmentSourceReadError::InvalidPersistedState {
        source: box_error(ProductListingContentAssessmentSourceMappingError {
            source: static_error(message),
        }),
    }
}

fn mapping_error_source(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProductListingContentAssessmentSourceReadError {
    ProductListingContentAssessmentSourceReadError::InvalidPersistedState {
        source: box_error(ProductListingContentAssessmentSourceMappingError {
            source: box_error(source),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_normalize_empty_content_assessment_source_text_to_absence() {
        assert!(matches!(
            content_title(Some(String::new()), Some("en".to_owned())),
            Ok(None)
        ));
        assert!(matches!(
            content_description(Some(String::new()), Some("en".to_owned())),
            Ok(None)
        ));
    }

    #[test]
    fn should_reject_noncanonical_content_assessment_source_language() {
        assert!(parse_language("EN").is_err());
    }
}

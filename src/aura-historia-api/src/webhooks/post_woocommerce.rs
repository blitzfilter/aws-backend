use crate::auth::protected_context;
use crate::error::{ApiError, BAD_BODY_VALUE, BAD_HEADER_VALUE, INVALID_UUID};
use crate::state::WebhooksState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use listing_source_core::ListingSourceId;
use woocommerce_service::{WoocommerceProductEventKind, WoocommerceWebhookIntakeCommand};

const TOPIC_HEADER: &str = "x-wc-webhook-topic";
const SIGNATURE_HEADER: &str = "x-wc-webhook-signature";
const DELIVERY_ID_HEADER: &str = "x-wc-webhook-delivery-id";

pub async fn post_woocommerce(
    State(state): State<WebhooksState>,
    headers: HeaderMap,
    Path(raw_listing_source_id): Path<String>,
    body: Bytes,
) -> Response {
    let listing_source_id = match parse_listing_source_id(&raw_listing_source_id) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    if body.is_empty() {
        return ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Body cannot be empty.")
            .into_response();
    }
    let kind = match event_kind(&headers) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let signature = match signature(&headers) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let delivery_id = match delivery_id(&headers) {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    match state
        .intake
        .execute(
            &context,
            WoocommerceWebhookIntakeCommand {
                listing_source_id,
                kind,
                signature,
                raw_body: body.to_vec(),
                delivery_id,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ApiError> {
    uuid::Uuid::parse_str(value)
        .map(ListingSourceId::from)
        .map_err(|_| {
            ApiError::bad_request(INVALID_UUID)
                .with_path_field("listingSourceId")
                .with_detail("Path parameter 'listingSourceId' must be a UUID.")
        })
}

fn event_kind(headers: &HeaderMap) -> Result<WoocommerceProductEventKind, ApiError> {
    match headers
        .get(TOPIC_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        Some("product.created") => Ok(WoocommerceProductEventKind::Create),
        Some("product.updated") => Ok(WoocommerceProductEventKind::Update),
        Some("product.deleted") => Ok(WoocommerceProductEventKind::Delete),
        Some(_) => Err(ApiError::bad_request(BAD_HEADER_VALUE)
            .with_header_field(TOPIC_HEADER)
            .with_detail("WooCommerce topic is unsupported.")),
        None => Err(ApiError::bad_request(BAD_HEADER_VALUE)
            .with_header_field(TOPIC_HEADER)
            .with_detail("WooCommerce topic header is required.")),
    }
}

fn delivery_id(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    headers
        .get(DELIVERY_ID_HEADER)
        .map(|value| {
            value.to_str().map(str::to_owned).map_err(|_| {
                ApiError::bad_request(BAD_HEADER_VALUE)
                    .with_header_field(DELIVERY_ID_HEADER)
                    .with_detail("WooCommerce delivery ID must be valid header text.")
            })
        })
        .transpose()
}

fn signature(headers: &HeaderMap) -> Result<Vec<u8>, ApiError> {
    let encoded = headers
        .get(SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field(SIGNATURE_HEADER)
                .with_detail("WooCommerce signature header is required.")
        })?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| {
            ApiError::unauthorized(BAD_HEADER_VALUE)
                .with_header_field(SIGNATURE_HEADER)
                .with_detail("WooCommerce signature must be base64 encoded.")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn should_parse_listing_source_id() {
        let listing_source_id = ListingSourceId::new();

        let parsed = parse_listing_source_id(&listing_source_id.to_string());

        assert!(matches!(parsed, Ok(actual) if actual == listing_source_id));
    }

    #[test]
    fn should_reject_invalid_listing_source_id() {
        let error = parse_listing_source_id("not-a-uuid");

        assert!(matches!(error, Err(error) if error.code() == INVALID_UUID));
    }

    #[test]
    fn should_map_supported_woocommerce_topics() {
        for (topic, expected) in [
            ("product.created", WoocommerceProductEventKind::Create),
            ("product.updated", WoocommerceProductEventKind::Update),
            ("product.deleted", WoocommerceProductEventKind::Delete),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(TOPIC_HEADER, HeaderValue::from_static(topic));
            assert!(matches!(event_kind(&headers), Ok(actual) if actual == expected));
        }
    }

    #[test]
    fn should_reject_missing_or_unsupported_woocommerce_topic() {
        let missing = event_kind(&HeaderMap::new());
        assert!(matches!(missing, Err(error) if error.code() == BAD_HEADER_VALUE));

        let mut headers = HeaderMap::new();
        headers.insert(TOPIC_HEADER, HeaderValue::from_static("order.created"));
        let unsupported = event_kind(&headers);
        assert!(matches!(unsupported, Err(error) if error.code() == BAD_HEADER_VALUE));
    }

    #[test]
    fn should_read_optional_woocommerce_delivery_id() -> Result<(), Box<dyn std::error::Error>> {
        assert!(matches!(delivery_id(&HeaderMap::new()), Ok(None)));

        let mut valid_headers = HeaderMap::new();
        valid_headers.insert(
            DELIVERY_ID_HEADER,
            HeaderValue::from_static("delivery-identifier"),
        );
        assert!(matches!(
            delivery_id(&valid_headers),
            Ok(Some(value)) if value == "delivery-identifier"
        ));

        let mut invalid_headers = HeaderMap::new();
        invalid_headers.insert(DELIVERY_ID_HEADER, HeaderValue::from_bytes(&[0xff])?);
        assert!(matches!(
            delivery_id(&invalid_headers),
            Err(error) if error.code() == BAD_HEADER_VALUE
        ));
        Ok(())
    }

    #[test]
    fn should_decode_valid_woocommerce_signature() {
        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_static("c2lnbmF0dXJl"));

        assert!(matches!(signature(&headers), Ok(value) if value == b"signature"));
    }

    #[test]
    fn should_reject_missing_or_invalid_woocommerce_signature() {
        let missing = signature(&HeaderMap::new());
        assert!(matches!(missing, Err(error) if error.code() == BAD_HEADER_VALUE));

        let mut headers = HeaderMap::new();
        headers.insert(SIGNATURE_HEADER, HeaderValue::from_static("not-base64!"));
        let invalid = signature(&headers);
        assert!(matches!(invalid, Err(error) if error.code() == BAD_HEADER_VALUE));
    }
}

use crate::auth::RequestMetadata;
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, Method, header};
use axum::middleware::Next;
use axum::response::Response;
use std::time::Duration;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::sensitive_headers::SetSensitiveRequestHeadersLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

pub(crate) const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");
pub(crate) const CORRELATION_ID_HEADER: HeaderName = HeaderName::from_static("x-correlation-id");

const WOOCOMMERCE_DELIVERY_ID_HEADER: HeaderName =
    HeaderName::from_static("x-wc-webhook-delivery-id");
const MAX_CORRELATION_ID_LENGTH: usize = 128;
const MAX_REQUEST_BODY_BYTES: usize = 1_048_576;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn with_transport_middleware(router: Router) -> Router {
    router
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                tracing::info_span!(
                    "http.request",
                    method = %request.method(),
                    path = %request.uri().path(),
                    request_id = ?request.headers().get(&REQUEST_ID_HEADER),
                    correlation_id = ?request.headers().get(&CORRELATION_ID_HEADER),
                )
            }),
        )
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
            HeaderName::from_static("x-api-key"),
            HeaderName::from_static("x-wc-webhook-signature"),
        ]))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_REQUEST_BODY_BYTES))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PATCH,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    HeaderName::from_static("x-wc-webhook-topic"),
                    HeaderName::from_static("x-wc-webhook-signature"),
                    WOOCOMMERCE_DELIVERY_ID_HEADER,
                    CORRELATION_ID_HEADER,
                ])
                .expose_headers([REQUEST_ID_HEADER, CORRELATION_ID_HEADER]),
        )
        .layer(axum::middleware::from_fn(request_metadata))
}

async fn request_metadata(mut request: Request, next: Next) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    let correlation_id = request
        .headers()
        .get(&CORRELATION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_correlation_id(value))
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| request_id.clone());
    let metadata = RequestMetadata::new(request_id.clone(), correlation_id.clone());

    // Route extractors receive the same immutable metadata through the headers and extensions.
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        request.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    if let Ok(value) = HeaderValue::from_str(&correlation_id) {
        request.headers_mut().insert(CORRELATION_ID_HEADER, value);
    }
    request.extensions_mut().insert(metadata);

    let mut response = next.run(request).await;
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    if let Ok(value) = HeaderValue::from_str(&correlation_id) {
        response.headers_mut().insert(CORRELATION_ID_HEADER, value);
    }
    response
}

fn valid_correlation_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CORRELATION_ID_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    fn app() -> Router {
        with_transport_middleware(Router::new().route("/", get(|| async { "ok" })))
    }

    #[tokio::test]
    async fn should_generate_and_propagate_bounded_request_metadata() {
        let response = app()
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(StatusCode::OK, response.status());
        let request_id = response.headers()[&REQUEST_ID_HEADER].to_str().unwrap();
        assert!(uuid::Uuid::parse_str(request_id).is_ok());
        assert_eq!(request_id, response.headers()[&CORRELATION_ID_HEADER]);
    }

    #[tokio::test]
    async fn should_preserve_only_valid_bounded_correlation_ids() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(CORRELATION_ID_HEADER, "trace_123-abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!("trace_123-abc", response.headers()[&CORRELATION_ID_HEADER]);
    }

    #[tokio::test]
    async fn should_replace_invalid_correlation_ids() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(CORRELATION_ID_HEADER, "contains space")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.headers()[&REQUEST_ID_HEADER],
            response.headers()[&CORRELATION_ID_HEADER]
        );
    }

    #[tokio::test]
    async fn should_expose_correlation_headers_for_cors_requests() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::ORIGIN, "https://example.test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!("*", response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN]);
        assert!(
            response.headers()[header::ACCESS_CONTROL_EXPOSE_HEADERS]
                .to_str()
                .unwrap()
                .contains(REQUEST_ID_HEADER.as_str())
        );
    }

    #[tokio::test]
    async fn should_allow_woocommerce_delivery_id_for_cors_preflight() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/")
                    .header(header::ORIGIN, "https://example.test")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, Method::POST.as_str())
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        WOOCOMMERCE_DELIVERY_ID_HEADER.as_str(),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(StatusCode::OK, response.status());
        let allowed_headers = response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(allowed_headers.split(',').any(|value| {
            value
                .trim()
                .eq_ignore_ascii_case(WOOCOMMERCE_DELIVERY_ID_HEADER.as_str())
        }));
    }

    #[tokio::test]
    async fn should_reject_requests_above_the_body_limit() {
        let response = app()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/")
                    .header(header::CONTENT_LENGTH, MAX_REQUEST_BODY_BYTES + 1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(axum::http::StatusCode::PAYLOAD_TOO_LARGE, response.status());
    }
}

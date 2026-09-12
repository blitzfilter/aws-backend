mod health;
mod ready;
mod version;

use crate::state::OperationsState;
use axum::{Router, middleware::Next, routing::get};
use http::{StatusCode, header::CACHE_CONTROL};
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

pub(crate) fn router(state: OperationsState) -> Router {
    Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(ready::ready))
        .route("/version", get(version::version))
        .with_state(state)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(2),
        ))
        .layer(axum::middleware::from_fn(
            |request, next: Next| async move {
                let mut response = next.run(request).await;
                response
                    .headers_mut()
                    .insert(CACHE_CONTROL, http::HeaderValue::from_static("no-store"));
                response
            },
        ))
}

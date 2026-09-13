#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;

use crate::{
    SEQUIN_CDC_PATH, WorkerRuntime,
    cdc::{CdcIngestError, MAX_CDC_BODY_BYTES},
};
use axum::{
    Extension, Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
    time::Instant,
};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const CONNECTION_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_HTTP_REQUESTS: usize = 16;
const MAX_HTTP_CONNECTIONS: usize = 16;
const MAX_HTTP_HEADERS: usize = 32;
const MAX_HTTP_HEADER_BYTES: usize = 16 * 1024;

struct ServerGuard(crate::queue::RuntimeControl);
impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

pub(crate) async fn serve<S>(
    listener: TcpListener,
    runtime: WorkerRuntime,
    shutdown: S,
) -> Result<(), crate::WorkerRunError>
where
    S: Future<Output = ()> + Send,
{
    let control = runtime.control.clone();
    let _guard = ServerGuard(control.clone());
    let router = router(runtime);
    // Count actual accepted sockets, including headers and response writes, not just middleware.
    // Completed tasks are reaped before accepting again; dropping the owner aborts every child.
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);
    let result = loop {
        tokio::select! {
            biased;
            () = &mut shutdown => break Ok(()),
            () = control.cancelled() => break Ok(()),
            result = connections.join_next(), if !connections.is_empty() => {
                if matches!(result, Some(Err(_))) {
                    tracing::warn!(outcome = "http_connection_task_failed", "HTTP task failed; unconfirmed publication must be retried");
                }
            }
            accepted = listener.accept(), if connections.len() < MAX_HTTP_CONNECTIONS => {
                match accepted {
                    Ok((stream, _)) => {
                        connections.spawn(serve_connection(stream, router.clone(), control.clone(), Instant::now()));
                    }
                    Err(error) => break Err(crate::WorkerRunError::Accept(error)),
                }
            }
        }
    };
    drop(listener);
    control.shutdown();
    if tokio::time::timeout(CONNECTION_TIMEOUT, async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tracing::warn!(
            outcome = "http_shutdown_deadline",
            "aborting remaining HTTP connections"
        );
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    result
}

async fn serve_connection(
    stream: TcpStream,
    router: Router,
    control: crate::queue::RuntimeControl,
    accepted_at: Instant,
) {
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .keep_alive(false)
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_headers(MAX_HTTP_HEADERS)
        .max_buf_size(MAX_HTTP_HEADER_BYTES);
    let request_started = RequestStarted::default();
    let router = router.layer(Extension(request_started.clone()));
    let connection =
        builder.serve_connection(TokioIo::new(stream), TowerToHyperService::new(router));
    tokio::pin!(connection);
    let serving = async {
        tokio::select! {
            result = &mut connection => return result,
            () = control.cancelled() => {}
        }
        // Hyper can treat partial headers as an in-progress request. Nothing was dispatched,
        // so close these sockets now instead of letting slow headers delay shutdown.
        if !request_started.0.load(Ordering::Acquire) {
            return Ok(());
        }
        connection.as_mut().graceful_shutdown();
        connection.await
    };
    match tokio::time::timeout_at(accepted_at + CONNECTION_TIMEOUT, serving).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => tracing::debug!(
            outcome = "http_protocol_or_io_error",
            "HTTP connection closed"
        ),
        Err(_) => tracing::debug!(
            outcome = "http_connection_timeout",
            "HTTP connection deadline reached"
        ),
    }
}

#[derive(Clone, Default)]
struct RequestStarted(Arc<AtomicBool>);

#[derive(Clone)]
struct HttpState {
    runtime: WorkerRuntime,
    capacity: Arc<Semaphore>,
}

pub(crate) fn router(runtime: WorkerRuntime) -> Router {
    let state = HttpState {
        runtime,
        capacity: Arc::new(Semaphore::new(MAX_HTTP_REQUESTS)),
    };
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/admission", get(admission))
        .route("/state", get(operational_state))
        .route("/version", get(version))
        .route(SEQUIN_CDC_PATH, post(ingest))
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found\n") })
        .layer(DefaultBodyLimit::max(MAX_CDC_BODY_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            bounded_request,
        ))
        .with_state(state)
}

async fn bounded_request(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    if let Some(started) = request.extensions().get::<RequestStarted>() {
        started.0.store(true, Ordering::Release);
    }
    let operational = matches!(
        request.uri().path(),
        "/health" | "/ready" | "/admission" | "/state" | "/version"
    );
    let mut response = if state.runtime.control.stopping() && !operational {
        (StatusCode::SERVICE_UNAVAILABLE, "worker stopping\n").into_response()
    } else if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .is_some_and(|value| {
            value.to_str().is_ok_and(|value| {
                value
                    .parse::<u64>()
                    .is_ok_and(|length| length > MAX_CDC_BODY_BYTES as u64)
            })
        })
    {
        // Reject an oversized declared body before waiting for its first frame. Axum still
        // collects and limits the complete body (including chunked requests) below.
        (StatusCode::PAYLOAD_TOO_LARGE, "CDC limits exceeded\n").into_response()
    } else if let Ok(_permit) = state.capacity.try_acquire() {
        match tokio::time::timeout(HTTP_TIMEOUT, next.run(request)).await {
            Ok(response) => response,
            Err(_) => (StatusCode::REQUEST_TIMEOUT, "worker request timed out\n").into_response(),
        }
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "worker HTTP capacity exhausted\n",
        )
            .into_response()
    };
    // Apply even on errors, HEAD, overload and shutdown responses.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store, max-age=0"),
    );
    // Preserve the private webhook server's one-request connection contract, without partial reads.
    response.headers_mut().insert(
        header::CONNECTION,
        header::HeaderValue::from_static("close"),
    );
    response
}
async fn health(State(state): State<HttpState>) -> impl IntoResponse {
    if state.runtime.control.live() {
        (StatusCode::OK, "ok\n")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "consumer unavailable\n")
    }
}
async fn ready(State(state): State<HttpState>) -> impl IntoResponse {
    if state.runtime.control.ready() {
        (StatusCode::OK, "ready\n")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready\n")
    }
}
async fn admission(State(state): State<HttpState>) -> impl IntoResponse {
    if state.runtime.admitting() {
        (StatusCode::OK, "accepting\n")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not accepting\n")
    }
}

async fn version(State(state): State<HttpState>) -> Response {
    match &state.runtime.operational {
        Some(config) => Json(&config.identity).into_response(),
        None => (StatusCode::SERVICE_UNAVAILABLE, "identity unavailable\n").into_response(),
    }
}

async fn operational_state(State(state): State<HttpState>) -> impl IntoResponse {
    let runtime = &state.runtime;
    let lifecycle = if runtime.control.stopping() {
        "DRAINING"
    } else if runtime.admitting() {
        "RUNNING"
    } else {
        "UNCONFIGURED"
    };
    Json(serde_json::json!({
        "schema_version": 1,
        "lifecycle": lifecycle,
        "ingress_admission": runtime.admitting(),
        "consumer_live": runtime.control.live(),
        "consumer_ready": runtime.control.ready(),
        "identity": runtime.operational.as_ref().map(|config| &config.identity),
        "budgets": runtime.operational.as_ref().map(|config| serde_json::json!({
            "drain_seconds": config.drain.as_secs(),
            "external_stop_seconds": config.stop.as_secs(),
            "execution_seconds": config.execution.as_secs(),
            "http_seconds": CONNECTION_TIMEOUT.as_secs(),
        })),
    }))
}

async fn ingest(State(state): State<HttpState>, body: Bytes) -> impl IntoResponse {
    // Consumer outages must not couple durable ingress to downstream availability.
    if state.runtime.control.stopping() {
        return (StatusCode::SERVICE_UNAVAILABLE, "worker stopping\n");
    }
    let Ok(body) = std::str::from_utf8(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid CDC JSON\n");
    };
    match state.runtime.ingest_cdc_json(body).await {
        Ok(_) => (StatusCode::ACCEPTED, "accepted\n"),
        Err(CdcIngestError::InvalidJson(_)) => (StatusCode::BAD_REQUEST, "invalid CDC JSON\n"),
        Err(CdcIngestError::LimitExceeded) => {
            (StatusCode::PAYLOAD_TOO_LARGE, "CDC limits exceeded\n")
        }
        Err(_) => {
            tracing::warn!(
                outcome = "cdc_rejected",
                "CDC not acknowledged; Sequin must retry"
            );
            (StatusCode::SERVICE_UNAVAILABLE, "CDC publication failed\n")
        }
    }
}

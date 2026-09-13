use crate::{CronRuntimeConfig, scheduled_job::ActiveExecutionTracker};
use axum::{
    Router,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};

const STARTING: u8 = 0;
const READY: u8 = 1;
const DRAINING: u8 = 2;
const STOPPED: u8 = 3;

#[doc(hidden)]
pub struct RuntimeHealth {
    state: AtomicU8,
    config: CronRuntimeConfig,
    tracker: Arc<ActiveExecutionTracker>,
    execution_seconds: AtomicU64,
}

impl RuntimeHealth {
    pub(crate) fn new(config: CronRuntimeConfig, tracker: Arc<ActiveExecutionTracker>) -> Self {
        Self {
            state: AtomicU8::new(STARTING),
            config,
            tracker,
            execution_seconds: AtomicU64::new(0),
        }
    }
    pub(crate) fn set_execution_seconds(&self, seconds: u64) {
        self.execution_seconds.store(seconds, Ordering::Release);
    }
    pub(crate) fn ready(&self) {
        self.state.store(READY, Ordering::Release);
    }
    pub(crate) fn draining(&self) {
        self.tracker.stop_accepting();
        self.state.store(DRAINING, Ordering::Release);
    }
    pub(crate) fn stopped(&self) {
        self.state.store(STOPPED, Ordering::Release);
    }
    fn state_name(&self) -> &'static str {
        match self.state.load(Ordering::Acquire) {
            STARTING => "starting",
            READY => "ready",
            DRAINING => "draining",
            _ => "stopped",
        }
    }
    fn identity(&self) -> String {
        // Every string is a constant or a strictly validated configuration identifier.
        let sha = self
            .config
            .source_sha
            .as_ref()
            .map(|sha| format!("\"{sha}\""))
            .unwrap_or_else(|| "null".into());
        format!(
            "\"schema_version\":1,\"component\":\"aura-historia-cron\",\"stage\":\"{}\",\"source_sha\":{sha}",
            self.config.stage
        )
    }
}

#[doc(hidden)]
pub fn router(health: Arc<RuntimeHealth>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .route("/ops/status", get(status_handler))
        .route("/ops/version", get(version_handler))
        .with_state(health)
}

fn response(status: StatusCode, body: String, json: bool) -> axum::response::Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (
                header::CONTENT_TYPE,
                if json {
                    "application/json"
                } else {
                    "text/plain; charset=utf-8"
                },
            ),
        ],
        body,
    )
        .into_response()
}

async fn health_handler() -> axum::response::Response {
    response(StatusCode::OK, "ok\n".into(), false)
}
async fn ready_handler(State(health): State<Arc<RuntimeHealth>>) -> axum::response::Response {
    let ready = health.state.load(Ordering::Acquire) == READY;
    response(
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        if ready { "ready\n" } else { "not ready\n" }.into(),
        false,
    )
}
async fn version_handler(State(health): State<Arc<RuntimeHealth>>) -> axum::response::Response {
    response(StatusCode::OK, format!("{{{}}}", health.identity()), true)
}
async fn status_handler(State(health): State<Arc<RuntimeHealth>>) -> axum::response::Response {
    let state = health.state_name();
    let execution = match health.execution_seconds.load(Ordering::Acquire) {
        0 => "null".into(),
        seconds => seconds.to_string(),
    };
    response(
        StatusCode::OK,
        format!(
            "{{{},\"state\":\"{state}\",\"accepting\":{},\"active_executions\":{},\"drain_seconds\":{},\"stop_seconds\":{},\"execution_seconds\":{execution},\"cleanup_seconds\":5,\"runtime_teardown_seconds\":1}}",
            health.identity(),
            state == "ready",
            health.tracker.active(),
            health.config.shutdown_grace().as_secs(),
            health.config.stop_timeout().as_secs()
        ),
        true,
    )
}

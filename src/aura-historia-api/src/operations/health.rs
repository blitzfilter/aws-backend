use crate::{runtime::ApiLifecycleState, state::OperationsState};
use axum::{extract::State, response::IntoResponse};
use http::StatusCode;

pub(super) async fn health(State(state): State<OperationsState>) -> impl IntoResponse {
    if state.lifecycle.state() == ApiLifecycleState::Stopped {
        (StatusCode::SERVICE_UNAVAILABLE, "stopped\n")
    } else {
        (StatusCode::OK, "ok\n")
    }
}

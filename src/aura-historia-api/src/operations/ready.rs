use crate::state::OperationsState;
use axum::extract::State;
use http::StatusCode;

pub(super) async fn ready(State(state): State<OperationsState>) -> StatusCode {
    if state.lifecycle.is_ready() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

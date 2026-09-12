use crate::state::OperationsState;
use axum::{Json, extract::State, response::IntoResponse};
use serde::Serialize;

#[derive(Serialize)]
struct VersionData<'a> {
    commit_sha: &'a str,
    state: &'static str,
}

pub(super) async fn version(State(state): State<OperationsState>) -> impl IntoResponse {
    Json(VersionData {
        commit_sha: &state.commit_sha,
        state: state.lifecycle.state().as_str(),
    })
    .into_response()
}

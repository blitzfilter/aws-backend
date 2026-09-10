use crate::{
    auth::protected_context,
    error::{ApiError, PARTNERSHIP_INTERNAL_ERROR},
    state::PartnershipsState,
    wire::parse_path_object_id,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use partnership_core::partnership_id::PartnershipId;
use partnership_service::use_cases::commands::dissolve_partnership::DissolvePartnershipCommand;

pub(super) async fn delete_partnership(
    State(state): State<PartnershipsState>,
    headers: HeaderMap,
    Path(raw_partnership_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let partnership_id: PartnershipId =
        match parse_path_object_id(&raw_partnership_id, "partnershipId", "Partnership") {
            Ok(value) => value,
            Err(error) => return no_store(error.into_response()),
        };
    let Some(dissolve) = state.dissolve else {
        return no_store(
            ApiError::internal_server_error(PARTNERSHIP_INTERNAL_ERROR)
                .with_detail("Partnership dissolution is not configured.")
                .into_response(),
        );
    };

    match dissolve
        .execute(&context, DissolvePartnershipCommand { partnership_id })
        .await
    {
        Ok(_) => no_store(StatusCode::NO_CONTENT.into_response()),
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

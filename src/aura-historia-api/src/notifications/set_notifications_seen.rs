use super::types::{UpdateNotificationsSeenData, parse_json};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::NotificationsState;
use crate::wire::parse_body_object_id;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use notification_service::use_cases::commands::update_notifications_seen::UpdateNotificationsSeenCommand;

pub(super) async fn update_notifications(
    State(state): State<NotificationsState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let data: UpdateNotificationsSeenData = match parse_json(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let notification_ids = match data
        .notification_ids
        .into_iter()
        .map(|value| parse_body_object_id(&value, "notificationIds", "Notification"))
        .collect()
    {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };

    match state
        .update_notifications_seen
        .execute(
            &context,
            UpdateNotificationsSeenCommand {
                notification_ids,
                seen: data.seen,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

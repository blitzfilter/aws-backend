use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::NotificationsState;
use crate::wire::parse_path_object_id;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use notification_service::use_cases::commands::delete_notification::DeleteNotificationCommand;

pub(super) async fn delete_notification(
    State(state): State<NotificationsState>,
    headers: HeaderMap,
    Path(raw_notification_id): Path<String>,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };
    let notification_id =
        match parse_path_object_id(&raw_notification_id, "notificationId", "Notification") {
            Ok(value) => value,
            Err(error) => return error.into_response(),
        };

    match state
        .delete_notification
        .execute(&context, DeleteNotificationCommand { notification_id })
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

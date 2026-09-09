use super::types::{UpdateNotificationSeenData, parse_json};
use crate::auth::protected_context;
use crate::error::ApiError;
use crate::state::NotificationsState;
use crate::wire::parse_path_object_id;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use notification_service::use_cases::commands::update_notification_seen::UpdateNotificationSeenCommand;

pub(super) async fn update_notification(
    State(state): State<NotificationsState>,
    headers: HeaderMap,
    Path(raw_notification_id): Path<String>,
    body: String,
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
    let data: UpdateNotificationSeenData = match parse_json(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };

    match state
        .update_notification_seen
        .execute(
            &context,
            UpdateNotificationSeenCommand {
                notification_id,
                seen: data.seen,
            },
        )
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => ApiError::from(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{
        AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
    };
    use crate::state::NotificationsState;
    use application::error::box_error;
    use application::operation_context::OperationContext;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use notification_core::notification_id::NotificationId;
    use notification_service::ports::notification_list_reader::NotificationListReadError;
    use notification_service::use_cases::commands::delete_notification::{
        DeleteNotificationCommand, DeleteNotificationError, DeleteNotificationUseCase,
    };
    use notification_service::use_cases::commands::delete_notifications::{
        DeleteNotificationsCommand, DeleteNotificationsError, DeleteNotificationsUseCase,
    };
    use notification_service::use_cases::commands::update_all_notifications_seen::{
        UpdateAllNotificationsSeenCommand, UpdateAllNotificationsSeenError,
        UpdateAllNotificationsSeenUseCase,
    };
    use notification_service::use_cases::commands::update_notification_seen::UpdateNotificationSeenUseCase;
    use notification_service::use_cases::commands::update_notifications_seen::{
        UpdateNotificationsSeenCommand, UpdateNotificationsSeenError,
        UpdateNotificationsSeenUseCase,
    };
    use notification_service::use_cases::queries::list_notifications::{
        ListNotificationsError, ListNotificationsRequest, ListNotificationsResult,
        ListNotificationsUseCase,
    };
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use tower::ServiceExt;
    use user_core::user_id::UserId;

    #[derive(Clone)]
    struct FakeAuthenticator {
        user_id: UserId,
    }

    #[async_trait::async_trait]
    impl TokenAuthenticator for FakeAuthenticator {
        async fn authenticate(
            &self,
            _: &str,
            _: &RequestMetadata,
        ) -> Result<TransportPrincipal, AuthError> {
            Ok(TransportPrincipal::User {
                user_id: self.user_id,
                auth_method: AuthMethod::CognitoJwt,
                capabilities: BTreeSet::new(),
            })
        }
    }

    #[derive(Clone, Default)]
    struct FakeUpdateNotificationSeen {
        commands: Arc<Mutex<Vec<UpdateNotificationSeenCommand>>>,
    }

    #[async_trait::async_trait]
    impl UpdateNotificationSeenUseCase for FakeUpdateNotificationSeen {
        async fn execute(
            &self,
            _: &OperationContext,
            command: UpdateNotificationSeenCommand,
        ) -> Result<
            notification_service::use_cases::commands::update_notification_seen::UpdateNotificationSeenResult,
            notification_service::use_cases::commands::update_notification_seen::UpdateNotificationSeenError,
        >{
            lock(&self.commands).push(command);
            Ok(notification_service::use_cases::commands::update_notification_seen::UpdateNotificationSeenResult)
        }
    }

    struct UnusedList;
    #[async_trait::async_trait]
    impl ListNotificationsUseCase for UnusedList {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ListNotificationsRequest,
        ) -> Result<ListNotificationsResult, ListNotificationsError> {
            Err(ListNotificationsError::ReadFailed(
                NotificationListReadError::ReadFailed {
                    source: box_error(std::io::Error::other("unused")),
                },
            ))
        }
    }

    struct UnusedUpdateNotificationsSeen;
    #[async_trait::async_trait]
    impl UpdateNotificationsSeenUseCase for UnusedUpdateNotificationsSeen {
        async fn execute(
            &self,
            _: &OperationContext,
            _: UpdateNotificationsSeenCommand,
        ) -> Result<
            notification_service::use_cases::commands::update_notifications_seen::UpdateNotificationsSeenResult,
            UpdateNotificationsSeenError,
        >{
            Err(UpdateNotificationsSeenError::Forbidden)
        }
    }

    struct UnusedUpdateAllNotificationsSeen;
    #[async_trait::async_trait]
    impl UpdateAllNotificationsSeenUseCase for UnusedUpdateAllNotificationsSeen {
        async fn execute(
            &self,
            _: &OperationContext,
            _: UpdateAllNotificationsSeenCommand,
        ) -> Result<
            notification_service::use_cases::commands::update_all_notifications_seen::UpdateAllNotificationsSeenResult,
            UpdateAllNotificationsSeenError,
        >{
            Err(UpdateAllNotificationsSeenError::Forbidden)
        }
    }

    struct UnusedDeleteNotification;
    #[async_trait::async_trait]
    impl DeleteNotificationUseCase for UnusedDeleteNotification {
        async fn execute(
            &self,
            _: &OperationContext,
            _: DeleteNotificationCommand,
        ) -> Result<
            notification_service::use_cases::commands::delete_notification::DeleteNotificationResult,
            DeleteNotificationError,
        >{
            Err(DeleteNotificationError::Forbidden)
        }
    }

    struct UnusedDeleteNotifications;
    #[async_trait::async_trait]
    impl DeleteNotificationsUseCase for UnusedDeleteNotifications {
        async fn execute(
            &self,
            _: &OperationContext,
            _: DeleteNotificationsCommand,
        ) -> Result<
            notification_service::use_cases::commands::delete_notifications::DeleteNotificationsResult,
            DeleteNotificationsError,
        >{
            Err(DeleteNotificationsError::Forbidden)
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(value) => value,
            Err(error) => error.into_inner(),
        }
    }

    #[tokio::test]
    async fn should_update_seen_state_when_authenticated() {
        let user_id = UserId::new();
        let update_notification_seen = FakeUpdateNotificationSeen::default();
        let commands = Arc::clone(&update_notification_seen.commands);
        let app = Router::new()
            .route(
                "/{notification_id}",
                axum::routing::patch(update_notification),
            )
            .with_state(NotificationsState::new(
                Arc::new(UnusedList),
                Arc::new(update_notification_seen),
                Arc::new(UnusedUpdateNotificationsSeen),
                Arc::new(UnusedUpdateAllNotificationsSeen),
                Arc::new(UnusedDeleteNotification),
                Arc::new(UnusedDeleteNotifications),
                Arc::new(FakeAuthenticator { user_id }),
            ));
        let notification_id = NotificationId::new();
        let request = Request::builder()
            .method("PATCH")
            .uri(format!("/{notification_id}"))
            .header(header::AUTHORIZATION, "Bearer token")
            .body(Body::from(r#"{"seen":true}"#))
            .unwrap_or_else(|error| panic!("failed to build request: {error}"));

        let response = app
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("request failed: {error}"));

        assert_eq!(StatusCode::NO_CONTENT, response.status());
        assert_eq!(
            vec![UpdateNotificationSeenCommand {
                notification_id,
                seen: true,
            }],
            lock(&commands).clone()
        );
    }
}

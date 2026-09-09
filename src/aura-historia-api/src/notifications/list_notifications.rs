use super::types::NotificationData;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE};
use crate::pagination_data::JsonCursoredData;
use crate::state::NotificationsState;
use crate::wire::parse_query_object_id;
use application::pagination::{Cursor, CursoredResult};
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use localization::Language;

use notification_core::notification_id::NotificationId;
use notification_service::ports::notification_list_reader::NotificationListCursor;
use notification_service::use_cases::queries::list_notifications::ListNotificationsRequest;
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListNotificationsQuery {
    #[serde(default)]
    #[serde(with = "crate::wire::language")]
    language: Language,
    size: Option<u64>,
    search_after: Option<String>,
}

pub(super) async fn list_notifications(
    State(state): State<NotificationsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query = match serde_qs::from_str::<ListNotificationsQuery>(
        raw_query.as_deref().unwrap_or_default(),
    ) {
        Ok(query) => query,
        Err(error) => {
            return ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
                .with_detail(error.to_string())
                .into_response();
        }
    };
    let (limit, cursor) = match notification_cursor(query.size, query.search_after.as_deref()) {
        Ok(cursor) => cursor,
        Err(error) => return error.into_response(),
    };
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return *response,
    };

    match state
        .list_notifications
        .execute(
            &context,
            ListNotificationsRequest {
                languages: vec![query.language],
                cursor,
                limit,
            },
        )
        .await
    {
        Ok(result) => {
            let search_after = match result
                .next_cursor
                .map(notification_cursor_value)
                .transpose()
            {
                Ok(value) => value,
                Err(error) => return error.into_response(),
            };
            let response = Json(JsonCursoredData::<NotificationData>::from(CursoredResult {
                items: result
                    .items
                    .into_iter()
                    .map(|item| NotificationData::from((item, result.presentation_preferences)))
                    .collect(),
                cursor: Cursor {
                    size: u64::from(limit),
                    search_after,
                },
                total: None,
            }))
            .into_response();
            no_store(response)
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn notification_cursor(
    size: Option<u64>,
    search_after: Option<&str>,
) -> Result<(u32, Option<NotificationListCursor>), ApiError> {
    let limit = size.unwrap_or(21).clamp(1, 100) as u32;
    let cursor = search_after.map(parse_notification_cursor).transpose()?;
    Ok((limit, cursor))
}

fn parse_notification_cursor(value: &str) -> Result<NotificationListCursor, ApiError> {
    let value: Value = serde_json::from_str(value).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(error.to_string())
    })?;
    let Value::Array(values) = value else {
        return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(
                "searchAfter must be a JSON array containing timestamp and notification ID.",
            ));
    };
    let [Value::String(created), Value::String(notification_id)] = values.as_slice() else {
        return Err(ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail("searchAfter must contain an RFC3339 timestamp and Notification ID."));
    };
    let created = OffsetDateTime::parse(created, &Rfc3339).map_err(|error| {
        ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
            .with_query_field("searchAfter")
            .with_detail(error.to_string())
    })?;
    let notification_id: NotificationId =
        parse_query_object_id(notification_id, "searchAfter", "Notification")?;
    Ok(NotificationListCursor {
        created,
        notification_id,
    })
}

fn notification_cursor_value(cursor: NotificationListCursor) -> Result<Value, ApiError> {
    cursor
        .created
        .format(&Rfc3339)
        .map(|created| json!([created, cursor.notification_id]))
        .map_err(|_| {
            ApiError::internal_server_error(BAD_QUERY_PARAMETER_VALUE)
                .with_detail("Notification cursor failed internally.")
        })
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_notification_cursor_with_type_id() {
        let created = "2026-08-19T12:00:00Z";
        let notification_id = NotificationId::new();
        let cursor = parse_notification_cursor(&format!(r#"["{created}","{notification_id}"]"#));

        assert!(matches!(cursor, Ok(cursor) if cursor.notification_id == notification_id));
    }

    #[test]
    fn should_reject_non_array_notification_cursor() {
        let cursor = parse_notification_cursor(r#""not-a-cursor""#);

        assert!(cursor.is_err());
    }
}

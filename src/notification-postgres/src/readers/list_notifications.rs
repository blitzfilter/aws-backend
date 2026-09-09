use crate::mapping::{NotificationRow, mapping_error};
use application::error::box_error;
use notification_core::notification::Notification;
use notification_service::ports::notification_list_reader::{
    NotificationListCursor, NotificationListItem, NotificationListPage, NotificationListReadError,
    NotificationListReader,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use user_core::user_id::UserId;

#[derive(Debug, Clone)]
pub struct SqlxNotificationListReader {
    pool: PgPool,
}

impl SqlxNotificationListReader {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct NotificationListRow {
    notification_id: Option<uuid::Uuid>,
    user_id: Option<uuid::Uuid>,
    kind: Option<String>,
    origin_event_id: Option<uuid::Uuid>,
    product_listing_id: Option<uuid::Uuid>,
    user_search_filter_id: Option<uuid::Uuid>,
    partnership_application_id: Option<uuid::Uuid>,
    payload_version: Option<i16>,
    payload: Option<serde_json::Value>,
    seen: Option<bool>,
    created: Option<OffsetDateTime>,
    updated: Option<OffsetDateTime>,
    show_unassessed_or_sensitive_content: bool,
}

impl NotificationListRow {
    fn notification_row(self) -> Result<Option<NotificationRow>, NotificationListReadError> {
        let Self {
            notification_id,
            user_id,
            kind,
            origin_event_id,
            product_listing_id,
            user_search_filter_id,
            partnership_application_id,
            payload_version,
            payload,
            seen,
            created,
            updated,
            show_unassessed_or_sensitive_content: _,
        } = self;
        let Some(notification_id) = notification_id else {
            return Ok(None);
        };
        let Some(user_id) = user_id else {
            return Err(missing_notification_column("user_id"));
        };
        let Some(kind) = kind else {
            return Err(missing_notification_column("kind"));
        };
        let Some(payload_version) = payload_version else {
            return Err(missing_notification_column("payload_version"));
        };
        let Some(payload) = payload else {
            return Err(missing_notification_column("payload"));
        };
        let Some(seen) = seen else {
            return Err(missing_notification_column("seen"));
        };
        let Some(created) = created else {
            return Err(missing_notification_column("created"));
        };
        let Some(updated) = updated else {
            return Err(missing_notification_column("updated"));
        };

        Ok(Some(NotificationRow {
            notification_id,
            user_id,
            kind,
            origin_event_id,
            product_listing_id,
            user_search_filter_id,
            partnership_application_id,
            payload_version,
            payload,
            seen,
            created,
            updated,
        }))
    }
}

#[async_trait::async_trait]
impl NotificationListReader for SqlxNotificationListReader {
    async fn list_for_user(
        &self,
        user_id: UserId,
        cursor: Option<NotificationListCursor>,
        limit: u32,
    ) -> Result<NotificationListPage, NotificationListReadError> {
        let limit = limit.clamp(1, 100);
        let page_size =
            usize::try_from(limit).map_err(|source| NotificationListReadError::ReadFailed {
                source: box_error(source),
            })?;
        let rows = sqlx::query_as::<_, NotificationListRow>(
            "WITH notification_page AS (SELECT n.notification_id, n.user_id, n.kind, n.origin_event_id, n.product_listing_id, n.user_search_filter_id, n.partnership_application_id, n.payload_version, n.payload, n.seen, n.created, n.updated FROM notifications n WHERE n.user_id = $1 AND ($2::timestamptz IS NULL OR (n.created, n.notification_id) < ($2, $3)) ORDER BY n.created DESC, n.notification_id DESC LIMIT $4) SELECT p.notification_id, p.user_id, p.kind, p.origin_event_id, p.product_listing_id, p.user_search_filter_id, p.partnership_application_id, p.payload_version, p.payload, p.seen, p.created, p.updated, u.show_unassessed_or_sensitive_content FROM users u LEFT JOIN notification_page p ON TRUE WHERE u.user_id = $1 ORDER BY p.created DESC NULLS LAST, p.notification_id DESC NULLS LAST",
        )
        .bind(user_id.into_uuid())
        .bind(cursor.map(|cursor| cursor.created))
        .bind(cursor.map(|cursor| cursor.notification_id.into_uuid()))
        .bind(i64::from(limit) + 1)
        .fetch_all(&self.pool)
        .await
        .map_err(|source| NotificationListReadError::ReadFailed {
            source: box_error(source),
        })?;

        let show_unassessed_or_sensitive_content = rows
            .first()
            .is_some_and(|row| row.show_unassessed_or_sensitive_content);
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(row) = row.notification_row()? else {
                continue;
            };
            let created = row.created;
            let updated = row.updated;
            let seen = row.seen;
            let notification = Notification::try_from(row).map_err(|error| {
                NotificationListReadError::InvalidReadModel {
                    source: mapping_error(error),
                }
            })?;
            let notification_id = notification.notification_id();
            let content = notification.content().clone();
            items.push(NotificationListItem {
                notification_id,
                content,
                seen,
                created,
                updated,
            });
        }

        let has_more = items.len() > page_size;
        if has_more {
            let _ = items.pop();
        }
        let next_cursor = if has_more {
            items.last().map(|item| NotificationListCursor {
                created: item.created,
                notification_id: item.notification_id,
            })
        } else {
            None
        };
        Ok(NotificationListPage {
            items,
            next_cursor,
            show_unassessed_or_sensitive_content,
        })
    }
}

fn missing_notification_column(column: &'static str) -> NotificationListReadError {
    NotificationListReadError::InvalidReadModel {
        source: box_error(std::io::Error::other(format!(
            "notification list row is missing {column}"
        ))),
    }
}

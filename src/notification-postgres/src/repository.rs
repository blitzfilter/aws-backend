use crate::mapping::{NotificationWriteValues, PAYLOAD_VERSION, mapping_error};
use application::error::box_error;
use platform_postgres::SqlxTransaction;

use notification_service::ports::{
    notification_creator::{
        NewNotification, NotificationCreationError, NotificationCreationOutcome,
    },
    notification_repository::{NotificationRepository, NotificationRepositoryFactory},
};
use sqlx::{PgConnection, Postgres, QueryBuilder};
use std::collections::HashSet;

const INSERT_CHUNK_SIZE: usize = 500;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxNotificationRepositoryFactory;

struct SqlxNotificationRepository<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxNotificationRepositoryFactory {
    pub fn new() -> Self {
        Self
    }
}

impl NotificationRepositoryFactory<SqlxTransaction> for SqlxNotificationRepositoryFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl NotificationRepository + 'tx {
        SqlxNotificationRepository {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl NotificationRepository for SqlxNotificationRepository<'_> {
    async fn insert_many(
        &mut self,
        notifications: &[NewNotification],
    ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
        if notifications.is_empty() {
            return Ok(Vec::new());
        }
        let values = notifications
            .iter()
            .map(|item| NotificationWriteValues::try_from(&item.notification))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| NotificationCreationError::CreateFailed {
                source: mapping_error(error),
            })?;
        let mut inserted = HashSet::new();
        for values in values.chunks(INSERT_CHUNK_SIZE) {
            inserted.extend(insert_notifications(self.connection, values).await?);
        }
        Ok(notifications
            .iter()
            .map(|item| {
                let id = item.notification.notification_id().into_uuid();
                if inserted.contains(&id) {
                    NotificationCreationOutcome::Inserted {
                        notification_id: item.notification.notification_id(),
                    }
                } else {
                    NotificationCreationOutcome::Duplicate
                }
            })
            .collect())
    }
}

async fn insert_notifications(
    connection: &mut PgConnection,
    values: &[NotificationWriteValues],
) -> Result<Vec<uuid::Uuid>, NotificationCreationError> {
    let mut inserted = Vec::new();
    for kind in [
        "WATCHLIST_PRICE_CHANGED",
        "WATCHLIST_AVAILABILITY_CHANGED",
        "SEARCH_FILTER_MATCH",
        "PARTNERSHIP_APPLICATION_APPROVED",
        "PARTNERSHIP_APPLICATION_REJECTED",
    ] {
        let group = values
            .iter()
            .filter(|value| value.kind == kind)
            .collect::<Vec<_>>();
        if group.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO notifications (notification_id, user_id, kind, origin_event_id, product_listing_id, user_search_filter_id, partnership_application_id, payload_version, payload, seen) ",
        );
        query.push_values(group, |mut row, value| {
            row.push_bind(value.notification_id)
                .push_bind(value.user_id)
                .push_bind(value.kind)
                .push_bind(value.origin_event_id)
                .push_bind(value.product_listing_id)
                .push_bind(value.user_search_filter_id)
                .push_bind(value.partnership_application_id)
                .push_bind(PAYLOAD_VERSION)
                .push_bind(&value.payload)
                .push_bind(false);
        });
        match kind {
            "WATCHLIST_PRICE_CHANGED" | "WATCHLIST_AVAILABILITY_CHANGED" => query.push(" ON CONFLICT (user_id, origin_event_id, kind) WHERE kind IN ('WATCHLIST_PRICE_CHANGED', 'WATCHLIST_AVAILABILITY_CHANGED') DO NOTHING"),
            "SEARCH_FILTER_MATCH" => query.push(" ON CONFLICT (user_id, user_search_filter_id, product_listing_id, origin_event_id) WHERE kind = 'SEARCH_FILTER_MATCH' DO NOTHING"),
            "PARTNERSHIP_APPLICATION_APPROVED" | "PARTNERSHIP_APPLICATION_REJECTED" => query.push(" ON CONFLICT (user_id, partnership_application_id) WHERE kind IN ('PARTNERSHIP_APPLICATION_APPROVED', 'PARTNERSHIP_APPLICATION_REJECTED') DO NOTHING"),
            _ => return Err(NotificationCreationError::CreateFailed {
                source: box_error(std::io::Error::other("unsupported notification kind")),
            }),
        };
        query.push(" RETURNING notification_id");
        let ids = query
            .build_query_scalar::<uuid::Uuid>()
            .fetch_all(&mut *connection)
            .await
            .map_err(|source| NotificationCreationError::CreateFailed {
                source: box_error(source),
            })?;
        inserted.extend(ids);
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transaction::{Transaction, UnitOfWork};
    use notification_core::{
        notification::{
            Notification, NotificationContent, PartnershipApplicationDecision,
            PartnershipApplicationNotificationSnapshot,
        },
        notification_id::NotificationId,
    };
    use notification_service::ports::notification_creator::{
        ExternalDeliveryRequest, NewNotification,
    };
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use party_core::party_name::PartyName;
    use platform_postgres::SqlxUnitOfWork;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use user_core::user_id::UserId;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_round_trip_partnership_application_notification_through_repository() {
        let pool = get_postgres_client().await;
        let user_id = UserId::new();
        let inserted_user = sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@example.test"))
        .execute(&pool)
        .await;
        assert!(inserted_user.is_ok());
        let notification = Notification::new(
            NotificationId::new(),
            user_id,
            NotificationContent::PartnershipApplication {
                partnership_application_id: PartnershipApplicationId::new(),
                snapshot: PartnershipApplicationNotificationSnapshot {
                    party_name: PartyName::try_from("Northwind Antiques")
                        .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
                    listing_source_name: listing_source_core::ListingSourceName::try_from(
                        "Northwind Source",
                    )
                    .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
                    image: None,
                },
                decision: PartnershipApplicationDecision::Approved,
            },
        );
        let mut transaction = match SqlxUnitOfWork::new(pool.clone()).begin().await {
            Ok(transaction) => transaction,
            Err(error) => panic!("begin transaction: {error}"),
        };
        let outcomes = match SqlxNotificationRepositoryFactory::new()
            .in_transaction(&mut transaction)
            .insert_many(&[NewNotification {
                notification: notification.clone(),
                external_delivery: ExternalDeliveryRequest::Requested,
            }])
            .await
        {
            Ok(outcomes) => outcomes,
            Err(error) => panic!("insert notification: {error}"),
        };
        let committed = transaction.commit().await;
        assert!(committed.is_ok());
        assert!(matches!(
            outcomes.as_slice(),
            [NotificationCreationOutcome::Inserted { notification_id }]
                if *notification_id == notification.notification_id()
        ));

        let row = match sqlx::query_as::<_, crate::mapping::NotificationRow>(
            "SELECT notification_id, user_id, kind, origin_event_id, product_listing_id, user_search_filter_id, partnership_application_id, payload_version, payload, seen, created, updated FROM notifications WHERE notification_id = $1",
        )
        .bind(notification.notification_id().into_uuid())
        .fetch_one(&pool)
        .await
        {
            Ok(row) => row,
            Err(error) => panic!("load notification: {error}"),
        };

        let restored = match Notification::try_from(row) {
            Ok(notification) => notification,
            Err(error) => panic!("map notification: {error}"),
        };
        assert_eq!(notification, restored);
    }
}

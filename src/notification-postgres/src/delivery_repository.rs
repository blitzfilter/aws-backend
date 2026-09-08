use crate::{
    delivery_mapping::channel_from_persisted,
    mapping::{NotificationRow, mapping_error},
};
use application::error::box_error;
use localization::Language;
use notification_core::{
    notification_delivery::NotificationDeliveryTargetKey,
    notification_delivery_id::NotificationDeliveryId, notification_id::NotificationId,
};
use notification_service::ports::notification_delivery_repository::{
    ClaimNotificationDeliveryOutcome, ClaimedNotificationDelivery, NotificationDeliveryError,
    NotificationDeliveryRepository, NotificationDeliverySource,
};
use notification_service::presentation::NotificationPresentationPreferences;
use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SqlxNotificationDeliveryRepository {
    pool: PgPool,
}

impl SqlxNotificationDeliveryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct DeliveryClaimRow {
    notification_delivery_id: Uuid,
    notification_id: Uuid,
    lease_token: Uuid,
    lease_expires_at: OffsetDateTime,
    attempt_count: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct DeliveryStatusRow {
    status: String,
    lease_token: Option<Uuid>,
    lease_expires_at: Option<OffsetDateTime>,
    observed_at: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
struct DeliverySourceRow {
    notification_delivery_id: Uuid,
    channel: String,
    target_key: String,
    language: Option<String>,
    show_unassessed_or_sensitive_content: bool,
    notification_id: Uuid,
    user_id: Uuid,
    kind: String,
    origin_event_id: Option<Uuid>,
    product_listing_id: Option<Uuid>,
    user_search_filter_id: Option<Uuid>,
    partnership_application_id: Option<Uuid>,
    payload_version: i16,
    payload: serde_json::Value,
    seen: bool,
    created: OffsetDateTime,
    updated: OffsetDateTime,
}

#[async_trait::async_trait]
impl NotificationDeliveryRepository for SqlxNotificationDeliveryRepository {
    async fn claim_and_load_source(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        now: OffsetDateTime,
        lease_expires_at: OffsetDateTime,
        lease_token: Uuid,
    ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
        let mut transaction = self.pool.begin().await.map_err(DeliveryOperationFailed)?;
        let row = sqlx::query_as::<_, DeliveryClaimRow>(
            "UPDATE notification_deliveries SET status = 'PROCESSING', lease_token = $2, lease_expires_at = $3, completed_lease_token = NULL, completed_at = NULL, attempt_count = attempt_count + 1, updated = now() WHERE notification_delivery_id = $1 AND $3 > GREATEST($4, clock_timestamp()) AND (status = 'PENDING' OR (status = 'PROCESSING' AND lease_expires_at <= GREATEST($4, clock_timestamp()))) RETURNING notification_delivery_id, notification_id, lease_token, lease_expires_at, attempt_count",
        )
        .bind(Uuid::from(notification_delivery_id))
        .bind(lease_token)
        .bind(lease_expires_at)
        .bind(now)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DeliveryOperationFailed)?;

        let Some(row) = row else {
            let outcome =
                load_unclaimed_outcome(&mut transaction, notification_delivery_id, now).await?;
            transaction
                .commit()
                .await
                .map_err(DeliveryOperationFailed)?;
            return Ok(outcome);
        };

        let claimed = claimed_from_row(row)?;
        let source = sqlx::query_as::<_, DeliverySourceRow>(
            "SELECT d.notification_delivery_id, d.channel, d.target_key, u.language, u.show_unassessed_or_sensitive_content, n.notification_id, n.user_id, n.kind, n.origin_event_id, n.product_listing_id, n.user_search_filter_id, n.partnership_application_id, n.payload_version, n.payload, n.seen, n.created, n.updated FROM notification_deliveries d JOIN notifications n ON n.notification_id = d.notification_id JOIN users u ON u.user_id = n.user_id WHERE d.notification_delivery_id = $1",
        )
        .bind(Uuid::from(notification_delivery_id))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DeliveryOperationFailed)?;
        let source = source.map(source_from_row).transpose()?;
        transaction
            .commit()
            .await
            .map_err(DeliveryOperationFailed)?;

        Ok(ClaimNotificationDeliveryOutcome::Claimed {
            delivery: claimed,
            source: Box::new(source),
        })
    }

    async fn mark_delivered(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        provider_message_id: &str,
        delivered_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(
            &self.pool,
            notification_delivery_id,
            lease_token,
            DeliveryCompletion::Delivered {
                provider_message_id,
            },
            delivered_at,
        )
        .await
    }

    async fn mark_retryable_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(
            &self.pool,
            notification_delivery_id,
            lease_token,
            DeliveryCompletion::RetryableFailure { error_code },
            completed_at,
        )
        .await
    }

    async fn mark_permanent_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: Uuid,
        error_code: &str,
        completed_at: OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        complete(
            &self.pool,
            notification_delivery_id,
            lease_token,
            DeliveryCompletion::PermanentFailure { error_code },
            completed_at,
        )
        .await
    }
}

struct DeliveryOperationFailed(sqlx::Error);

impl From<DeliveryOperationFailed> for NotificationDeliveryError {
    fn from(error: DeliveryOperationFailed) -> Self {
        Self::OperationFailed {
            source: box_error(error.0),
        }
    }
}

async fn load_unclaimed_outcome(
    connection: &mut PgConnection,
    id: NotificationDeliveryId,
    now: OffsetDateTime,
) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
    let row = sqlx::query_as::<_, DeliveryStatusRow>(
        "SELECT status, lease_token, lease_expires_at, GREATEST($2, clock_timestamp()) AS observed_at FROM notification_deliveries WHERE notification_delivery_id = $1",
    )
    .bind(Uuid::from(id))
    .bind(now)
    .fetch_optional(connection)
    .await
    .map_err(DeliveryOperationFailed)?;
    unclaimed_from_row(row)
}

fn unclaimed_from_row(
    row: Option<DeliveryStatusRow>,
) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
    let Some(row) = row else {
        return Ok(ClaimNotificationDeliveryOutcome::Missing);
    };
    if row.status == "PROCESSING" {
        let (Some(_), Some(lease_expires_at)) = (row.lease_token, row.lease_expires_at) else {
            return Err(invalid_delivery_source(
                "processing delivery has no complete lease",
            ));
        };
        return Ok(if lease_expires_at > row.observed_at {
            ClaimNotificationDeliveryOutcome::AlreadyClaimed { lease_expires_at }
        } else {
            ClaimNotificationDeliveryOutcome::Reclaimable
        });
    }
    if row.lease_token.is_some() || row.lease_expires_at.is_some() {
        return Err(invalid_delivery_source(
            "nonprocessing delivery has an active lease",
        ));
    }
    match row.status.as_str() {
        "DELIVERED" => Ok(ClaimNotificationDeliveryOutcome::Delivered),
        "FAILED" => Ok(ClaimNotificationDeliveryOutcome::PermanentlyFailed),
        "PENDING" => Ok(ClaimNotificationDeliveryOutcome::Reclaimable),
        _ => Err(invalid_delivery_source(
            "unknown notification delivery status",
        )),
    }
}

fn claimed_from_row(
    row: DeliveryClaimRow,
) -> Result<ClaimedNotificationDelivery, NotificationDeliveryError> {
    let attempt_count = u32::try_from(row.attempt_count).map_err(|source| {
        NotificationDeliveryError::InvalidPersistedState {
            source: box_error(source),
        }
    })?;
    Ok(ClaimedNotificationDelivery {
        notification_delivery_id: NotificationDeliveryId::from(row.notification_delivery_id),
        notification_id: NotificationId::from(row.notification_id),
        lease_token: row.lease_token,
        lease_expires_at: row.lease_expires_at,
        attempt_count,
    })
}

fn source_from_row(
    row: DeliverySourceRow,
) -> Result<NotificationDeliverySource, NotificationDeliveryError> {
    let notification = notification_core::notification::Notification::try_from(NotificationRow {
        notification_id: row.notification_id,
        user_id: row.user_id,
        kind: row.kind,
        origin_event_id: row.origin_event_id,
        product_listing_id: row.product_listing_id,
        user_search_filter_id: row.user_search_filter_id,
        partnership_application_id: row.partnership_application_id,
        payload_version: row.payload_version,
        payload: row.payload,
        seen: row.seen,
        created: row.created,
        updated: row.updated,
    })
    .map_err(|error| NotificationDeliveryError::InvalidPersistedState {
        source: mapping_error(error),
    })?;

    Ok(NotificationDeliverySource {
        notification_delivery_id: NotificationDeliveryId::from(row.notification_delivery_id),
        notification_id: notification.notification_id(),
        user_id: notification.user_id(),
        channel: channel_from_persisted(&row.channel).map_err(|source| {
            NotificationDeliveryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?,
        target_key: NotificationDeliveryTargetKey::try_from(row.target_key).map_err(|source| {
            NotificationDeliveryError::InvalidPersistedState {
                source: box_error(source),
            }
        })?,
        content: notification.content().clone(),
        presentation_preferences: NotificationPresentationPreferences {
            language: row
                .language
                .as_deref()
                .map(parse_language)
                .transpose()?
                .unwrap_or(Language::En),
            show_unassessed_or_sensitive_content: row.show_unassessed_or_sensitive_content,
        },
    })
}

fn parse_language(value: &str) -> Result<Language, NotificationDeliveryError> {
    Language::from_code(value).ok_or_else(|| invalid_delivery_source("unknown user language"))
}

fn invalid_delivery_source(message: &'static str) -> NotificationDeliveryError {
    NotificationDeliveryError::InvalidPersistedState {
        source: box_error(std::io::Error::other(message)),
    }
}

#[derive(Clone, Copy)]
enum DeliveryCompletion<'a> {
    Delivered { provider_message_id: &'a str },
    RetryableFailure { error_code: &'a str },
    PermanentFailure { error_code: &'a str },
}

async fn complete(
    pool: &PgPool,
    id: NotificationDeliveryId,
    lease_token: Uuid,
    completion: DeliveryCompletion<'_>,
    completed_at: OffsetDateTime,
) -> Result<bool, NotificationDeliveryError> {
    let (status, provider_message_id, error_code, delivered_at) = match completion {
        DeliveryCompletion::Delivered {
            provider_message_id,
        } => (
            "DELIVERED",
            Some(provider_message_id),
            None,
            Some(completed_at),
        ),
        DeliveryCompletion::RetryableFailure { error_code } => {
            ("PENDING", None, Some(error_code), None)
        }
        DeliveryCompletion::PermanentFailure { error_code } => {
            ("FAILED", None, Some(error_code), None)
        }
    };
    let result = sqlx::query(
        "UPDATE notification_deliveries SET status = $3, lease_token = NULL, lease_expires_at = NULL, completed_lease_token = $2, completed_at = $4, provider_message_id = $5, last_error_code = $6, delivered_at = $7, updated = now() WHERE notification_delivery_id = $1 AND status = 'PROCESSING' AND lease_token = $2 AND lease_expires_at > $4 AND lease_expires_at > clock_timestamp()",
    )
    .bind(Uuid::from(id))
    .bind(lease_token)
    .bind(status)
    .bind(completed_at)
    .bind(provider_message_id)
    .bind(error_code)
    .bind(delivered_at)
    .execute(pool)
    .await
    .map_err(DeliveryOperationFailed)?;
    if result.rows_affected() == 1 {
        return Ok(true);
    }

    // Use a fresh statement snapshot: a competing identical finalization may have
    // committed while UPDATE waited. An exact replay is read-only, even after expiry.
    // Reclaim clears the receipt, so an old token cannot bless a newer attempt.
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM notification_deliveries WHERE notification_delivery_id = $1 AND status = $3 AND completed_lease_token = $2 AND completed_at = $4 AND provider_message_id IS NOT DISTINCT FROM $5 AND last_error_code IS NOT DISTINCT FROM $6 AND delivered_at IS NOT DISTINCT FROM $7)",
    )
    .bind(Uuid::from(id))
    .bind(lease_token)
    .bind(status)
    .bind(completed_at)
    .bind(provider_message_id)
    .bind(error_code)
    .bind(delivered_at)
    .fetch_one(pool)
    .await
    .map_err(DeliveryOperationFailed)
    .map_err(NotificationDeliveryError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::BoxError;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use time::Duration;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    type TestResult = Result<(), BoxError>;

    #[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
    struct PersistedDeliveryRow {
        status: String,
        attempt_count: i32,
        lease_token: Option<Uuid>,
        lease_expires_at: Option<OffsetDateTime>,
        completed_lease_token: Option<Uuid>,
        completed_at: Option<OffsetDateTime>,
        provider_message_id: Option<String>,
        last_error_code: Option<String>,
        delivered_at: Option<OffsetDateTime>,
        updated: OffsetDateTime,
    }

    async fn persisted(
        pool: &PgPool,
        id: NotificationDeliveryId,
    ) -> Result<PersistedDeliveryRow, sqlx::Error> {
        sqlx::query_as("SELECT status, attempt_count, lease_token, lease_expires_at, completed_lease_token, completed_at, provider_message_id, last_error_code, delivered_at, updated FROM notification_deliveries WHERE notification_delivery_id = $1")
            .bind(Uuid::from(id)).fetch_one(pool).await
    }

    async fn seed(pool: &PgPool) -> Result<NotificationDeliveryId, sqlx::Error> {
        let user_id = Uuid::now_v7();
        let notification_id = Uuid::now_v7();
        let delivery_id = NotificationDeliveryId::new();
        sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(user_id)
        .bind(format!("{user_id}@example.test"))
        .execute(pool)
        .await?;
        sqlx::query("INSERT INTO notifications (notification_id, user_id, kind, partnership_application_id, payload_version, payload) VALUES ($1, $2, 'PARTNERSHIP_APPLICATION_APPROVED', $3, 1, $4)")
            .bind(notification_id).bind(user_id).bind(Uuid::now_v7())
            .bind(serde_json::json!({"type": "PARTNERSHIP_APPLICATION", "snapshot": {"party_name": "Test Party", "listing_source_name": "Test Source", "image": null}}))
            .execute(pool).await?;
        sqlx::query("INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key) VALUES ($1, $2, 'EMAIL', 'PRIMARY')")
            .bind(Uuid::from(delivery_id)).bind(notification_id).execute(pool).await?;
        Ok(delivery_id)
    }

    async fn claim(
        repository: &SqlxNotificationDeliveryRepository,
        id: NotificationDeliveryId,
        now: OffsetDateTime,
    ) -> Result<ClaimedNotificationDelivery, BoxError> {
        into_claimed(
            repository
                .claim_and_load_source(id, now, now + Duration::minutes(5), Uuid::now_v7())
                .await?,
        )
    }

    fn into_claimed(
        outcome: ClaimNotificationDeliveryOutcome,
    ) -> Result<ClaimedNotificationDelivery, BoxError> {
        match outcome {
            ClaimNotificationDeliveryOutcome::Claimed { delivery, source } => {
                assert!(source.is_some());
                Ok(delivery)
            }
            _ => Err(box_error(std::io::Error::other(
                "expected a claimed delivery with source",
            ))),
        }
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_allow_only_one_concurrent_claim_and_return_the_winners_persisted_expiry() {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let id = seed(&pool).await?;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            let now = OffsetDateTime::now_utc();
            let (left, right) = tokio::join!(
                repository.claim_and_load_source(
                    id,
                    now,
                    now + Duration::minutes(5),
                    Uuid::now_v7()
                ),
                repository.claim_and_load_source(
                    id,
                    now,
                    now + Duration::minutes(4),
                    Uuid::now_v7()
                ),
            );
            let outcomes = [left?, right?];
            let stored = persisted(&pool, id).await?;
            assert_eq!("PROCESSING", stored.status);
            assert_eq!(1, stored.attempt_count);
            let mut claims = 0;
            let mut deferrals = 0;
            for outcome in outcomes {
                match outcome {
                    ClaimNotificationDeliveryOutcome::Claimed { delivery, source } => {
                        claims += 1;
                        assert!(source.is_some());
                        assert_eq!(Some(delivery.lease_token), stored.lease_token);
                        assert_eq!(Some(delivery.lease_expires_at), stored.lease_expires_at);
                    }
                    ClaimNotificationDeliveryOutcome::AlreadyClaimed { lease_expires_at } => {
                        deferrals += 1;
                        assert_eq!(Some(lease_expires_at), stored.lease_expires_at);
                    }
                    _ => {
                        return Err(box_error(std::io::Error::other(
                            "unexpected concurrent claim result",
                        )));
                    }
                }
            }
            assert_eq!((1, 1), (claims, deferrals));
            Ok(())
        }
        .await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_defer_before_expiry_and_reclaim_at_or_after_expiry_with_new_token() {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            for after_expiry in [Duration::ZERO, Duration::seconds(1)] {
                let id = seed(&pool).await?;
                let now = OffsetDateTime::now_utc();
                let first = claim(&repository, id, now).await?;
                let before = first.lease_expires_at - Duration::seconds(1);
                assert_eq!(
                    ClaimNotificationDeliveryOutcome::AlreadyClaimed {
                        lease_expires_at: first.lease_expires_at
                    },
                    repository
                        .claim_and_load_source(
                            id,
                            before,
                            before + Duration::minutes(5),
                            Uuid::now_v7()
                        )
                        .await?,
                );
                let second = claim(&repository, id, first.lease_expires_at + after_expiry).await?;
                assert_ne!(first.lease_token, second.lease_token);
                assert_eq!(2, second.attempt_count);
                let stored = persisted(&pool, id).await?;
                assert_eq!(Some(second.lease_token), stored.lease_token);
                assert_eq!(Some(second.lease_expires_at), stored.lease_expires_at);
                for completion in [
                    DeliveryCompletion::Delivered {
                        provider_message_id: "stale-receipt",
                    },
                    DeliveryCompletion::RetryableFailure {
                        error_code: "STALE_RETRY",
                    },
                    DeliveryCompletion::PermanentFailure {
                        error_code: "STALE_FAILURE",
                    },
                ] {
                    assert!(!complete(&pool, id, first.lease_token, completion, now).await?);
                    assert_eq!(stored, persisted(&pool, id).await?);
                }
                assert!(
                    repository
                        .mark_delivered(
                            id,
                            second.lease_token,
                            "new-receipt",
                            second.lease_expires_at - Duration::seconds(1)
                        )
                        .await?
                );
                assert!(
                    !repository
                        .mark_delivered(id, first.lease_token, "new-receipt", now)
                        .await?
                );
            }
            Ok(())
        }
        .await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_reject_late_finalization_even_when_original_completion_preceded_expiry() {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            let id = seed(&pool).await?;
            let now = OffsetDateTime::now_utc();
            let first = claim(&repository, id, now).await?;
            sqlx::query("UPDATE notification_deliveries SET lease_expires_at = $2 WHERE notification_delivery_id = $1")
                .bind(Uuid::from(id)).bind(now - Duration::seconds(1)).execute(&pool).await?;
            let stored = persisted(&pool, id).await?;
            for completion in [
                DeliveryCompletion::Delivered { provider_message_id: "accepted-before-expiry" },
                DeliveryCompletion::RetryableFailure { error_code: "TEMPORARY" },
                DeliveryCompletion::PermanentFailure { error_code: "PERMANENT" },
            ] {
                assert!(!complete(&pool, id, first.lease_token, completion, now - Duration::seconds(2)).await?);
                assert_eq!(stored, persisted(&pool, id).await?);
            }
            Ok(())
        }.await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_replay_only_exact_committed_token_result_and_timestamp_without_another_write() {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            for (completion, different, expected_status) in [
                (
                    DeliveryCompletion::Delivered {
                        provider_message_id: "receipt",
                    },
                    DeliveryCompletion::Delivered {
                        provider_message_id: "different",
                    },
                    "DELIVERED",
                ),
                (
                    DeliveryCompletion::RetryableFailure {
                        error_code: "TEMPORARY",
                    },
                    DeliveryCompletion::RetryableFailure {
                        error_code: "DIFFERENT",
                    },
                    "PENDING",
                ),
                (
                    DeliveryCompletion::PermanentFailure {
                        error_code: "PERMANENT",
                    },
                    DeliveryCompletion::PermanentFailure {
                        error_code: "DIFFERENT",
                    },
                    "FAILED",
                ),
            ] {
                let id = seed(&pool).await?;
                let first = claim(&repository, id, OffsetDateTime::now_utc()).await?;
                // Deliberately keep nanoseconds: retries compare DB-normalized bind values.
                let completed_at = OffsetDateTime::now_utc().replace_nanosecond(123_456_789)?;
                assert!(complete(&pool, id, first.lease_token, completion, completed_at).await?);
                let stored = persisted(&pool, id).await?;
                assert_eq!(expected_status, stored.status);
                assert_eq!(Some(first.lease_token), stored.completed_lease_token);
                assert_eq!(
                    Some(123_456),
                    stored.completed_at.map(|at| at.microsecond())
                );
                assert!(stored.lease_token.is_none());
                assert!(stored.lease_expires_at.is_none());
                // Simulate the caller losing the successful response and retrying.
                assert!(complete(&pool, id, first.lease_token, completion, completed_at).await?);
                assert!(!complete(&pool, id, Uuid::now_v7(), completion, completed_at).await?);
                assert!(!complete(&pool, id, first.lease_token, different, completed_at).await?);
                assert!(
                    !complete(
                        &pool,
                        id,
                        first.lease_token,
                        completion,
                        completed_at + Duration::microseconds(1)
                    )
                    .await?
                );
                assert_eq!(stored, persisted(&pool, id).await?);
            }
            Ok(())
        }
        .await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_recognize_concurrent_identical_finalizations_and_replay_after_original_expiry()
    {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            let id = seed(&pool).await?;
            let now = OffsetDateTime::now_utc();
            let claimed = into_claimed(
                repository
                    .claim_and_load_source(id, now, now + Duration::seconds(1), Uuid::now_v7())
                    .await?,
            )?;
            let completed_at = OffsetDateTime::now_utc();
            let (left, right) = tokio::join!(
                repository.mark_delivered(id, claimed.lease_token, "receipt", completed_at),
                repository.mark_delivered(id, claimed.lease_token, "receipt", completed_at),
            );
            assert!(left?);
            assert!(right?);
            let stored = persisted(&pool, id).await?;
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
            assert!(
                repository
                    .mark_delivered(id, claimed.lease_token, "receipt", completed_at)
                    .await?
            );
            assert_eq!(stored, persisted(&pool, id).await?);
            assert_eq!(
                ClaimNotificationDeliveryOutcome::Delivered,
                repository
                    .claim_and_load_source(
                        id,
                        OffsetDateTime::now_utc(),
                        OffsetDateTime::now_utc() + Duration::minutes(5),
                        Uuid::now_v7()
                    )
                    .await?
            );
            Ok(())
        }
        .await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_clear_previous_receipt_on_reclaim_and_fence_old_completion_against_new_result()
    {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            let id = seed(&pool).await?;
            let now = OffsetDateTime::now_utc();
            let first = claim(&repository, id, now).await?;
            assert!(
                repository
                    .mark_retryable_failure(id, first.lease_token, "RETRY", now)
                    .await?
            );
            let second = claim(&repository, id, now).await?;
            let stored = persisted(&pool, id).await?;
            assert!(stored.completed_lease_token.is_none());
            assert!(stored.completed_at.is_none());
            assert!(
                !repository
                    .mark_retryable_failure(id, first.lease_token, "RETRY", now)
                    .await?
            );
            assert_eq!(stored, persisted(&pool, id).await?);
            assert!(
                repository
                    .mark_retryable_failure(id, second.lease_token, "RETRY", now)
                    .await?
            );
            assert!(
                !repository
                    .mark_retryable_failure(id, first.lease_token, "RETRY", now)
                    .await?
            );
            assert!(
                repository
                    .mark_retryable_failure(id, second.lease_token, "RETRY", now)
                    .await?
            );
            assert_eq!(
                Some(second.lease_token),
                persisted(&pool, id).await?.completed_lease_token
            );
            Ok(())
        }
        .await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_classify_fresh_status_when_completion_races_with_unsuccessful_claim() {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            for (completion, expected) in [
                (DeliveryCompletion::RetryableFailure { error_code: "RETRY" }, ClaimNotificationDeliveryOutcome::Reclaimable),
                (DeliveryCompletion::PermanentFailure { error_code: "PERMANENT" }, ClaimNotificationDeliveryOutcome::PermanentlyFailed),
                (DeliveryCompletion::Delivered { provider_message_id: "receipt" }, ClaimNotificationDeliveryOutcome::Delivered),
            ] {
                let id = seed(&pool).await?;
                let now = OffsetDateTime::now_utc();
                let claimed = claim(&repository, id, now).await?;
                let mut transaction = pool.begin().await?;
                // The claim UPDATE sees an active lease. Another connection completes
                // before the adapter's fresh status read in this READ COMMITTED transaction.
                let missed = sqlx::query("UPDATE notification_deliveries SET attempt_count = attempt_count + 1 WHERE notification_delivery_id = $1 AND (status = 'PENDING' OR (status = 'PROCESSING' AND lease_expires_at <= $2))")
                    .bind(Uuid::from(id)).bind(now).execute(&mut *transaction).await?;
                assert_eq!(0, missed.rows_affected());
                assert!(complete(&pool, id, claimed.lease_token, completion, now).await?);
                assert_eq!(expected, load_unclaimed_outcome(&mut transaction, id, now).await?);
                transaction.commit().await?;
            }
            Ok(())
        }.await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_short_defer_expired_observed_lease_and_missing_row_without_inventing_expiry() {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            let now = OffsetDateTime::now_utc();
            let missing = NotificationDeliveryId::new();
            assert_eq!(
                ClaimNotificationDeliveryOutcome::Missing,
                repository
                    .claim_and_load_source(missing, now, now + Duration::minutes(5), Uuid::now_v7())
                    .await?
            );
            assert!(
                !repository
                    .mark_delivered(missing, Uuid::now_v7(), "receipt", now)
                    .await?
            );
            let id = seed(&pool).await?;
            let claimed = claim(&repository, id, now).await?;
            let mut connection = pool.acquire().await?;
            assert_eq!(
                ClaimNotificationDeliveryOutcome::Reclaimable,
                load_unclaimed_outcome(&mut connection, id, claimed.lease_expires_at).await?
            );
            // A claim that spent its entire proposed lease waiting must not start work.
            let pending = seed(&pool).await?;
            assert_eq!(
                ClaimNotificationDeliveryOutcome::Reclaimable,
                repository
                    .claim_and_load_source(
                        pending,
                        now - Duration::minutes(6),
                        now - Duration::minutes(1),
                        Uuid::now_v7()
                    )
                    .await?
            );
            assert_eq!(0, persisted(&pool, pending).await?.attempt_count);
            Ok(())
        }
        .await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_rollback_claim_when_source_mapping_fails() {
        let result: TestResult = async {
            let pool = get_postgres_client().await;
            let repository = SqlxNotificationDeliveryRepository::new(pool.clone());
            let id = seed(&pool).await?;
            sqlx::query("UPDATE notifications SET payload = '{}'::jsonb WHERE notification_id = (SELECT notification_id FROM notification_deliveries WHERE notification_delivery_id = $1)")
                .bind(Uuid::from(id)).execute(&pool).await?;
            let now = OffsetDateTime::now_utc();
            assert!(matches!(repository.claim_and_load_source(id, now, now + Duration::minutes(5), Uuid::now_v7()).await, Err(NotificationDeliveryError::InvalidPersistedState { .. })));
            let stored = persisted(&pool, id).await?;
            assert_eq!("PENDING", stored.status);
            assert_eq!(0, stored.attempt_count);
            assert!(stored.lease_token.is_none());
            Ok(())
        }.await;
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn should_reject_corrupt_status_or_lease_without_treating_it_as_acknowledgeable() {
        for (status, token, expiry) in [
            ("PROCESSING", None, Some(OffsetDateTime::UNIX_EPOCH)),
            ("PROCESSING", Some(Uuid::nil()), None),
            (
                "PENDING",
                Some(Uuid::nil()),
                Some(OffsetDateTime::UNIX_EPOCH),
            ),
            ("DELIVERED", None, Some(OffsetDateTime::UNIX_EPOCH)),
            ("processing", None, None),
            ("UNKNOWN", None, None),
        ] {
            assert!(matches!(
                unclaimed_from_row(Some(DeliveryStatusRow {
                    status: status.to_owned(),
                    lease_token: token,
                    lease_expires_at: expiry,
                    observed_at: OffsetDateTime::UNIX_EPOCH
                })),
                Err(NotificationDeliveryError::InvalidPersistedState { .. })
            ));
        }
    }

    #[test]
    fn should_retain_database_error_source_with_safe_display() {
        let error = NotificationDeliveryError::from(DeliveryOperationFailed(
            sqlx::Error::Protocol("private provider or recipient payload".to_owned()),
        ));
        assert_eq!("notification delivery operation failed", error.to_string());
        assert!(
            matches!(error, NotificationDeliveryError::OperationFailed { source } if source.downcast_ref::<sqlx::Error>().is_some())
        );
    }
}

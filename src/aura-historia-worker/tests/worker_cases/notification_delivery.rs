use aura_historia_worker::notification_delivery::consume_notification_delivery_queue;
use aura_historia_worker::{WorkerRunError, WorkerScope, serve_with_runtime};
use aws_sdk_s3::{
    Client as S3Client,
    config::Builder as S3ConfigBuilder,
    types::{BucketLocationConstraint, CreateBucketConfiguration},
};
use aws_sdk_sesv2::Client as SesClient;

use crate::{BUSINESS_SCHEMA, WORKER_ACCEPTANCE, WORKER_SEQUIN, support};
use application::error::box_error;
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use notification_core::{
    notification_delivery_id::NotificationDeliveryId, notification_id::NotificationId,
};
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::SqlxEmailDeliveryTargetReader;
use notification_postgres::SqlxNotificationDeliveryRepository;
use notification_service::{
    ports::{
        notification_channel_sender::{NotificationChannelSender, NotificationDeliveryDispatcher},
        notification_delivery_repository::{
            ClaimNotificationDeliveryOutcome, NotificationDeliveryError,
            NotificationDeliveryRepository,
        },
    },
    use_cases::commands::deliver_notification::{
        DeliverNotificationHandler, DeliverNotificationUseCase,
    },
};
use product_listing_core::product_listing_id::ProductListingId;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use test_api::{
    IntegrationTestService, S3, Ses, aura_integration_test, get_postgres_client, get_sent_emails,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use user_core::user_id::UserId;

const SCOPE: WorkerScope = WorkerScope::NotificationDelivery;
const WORKER_SQS: test_api::WorkerSqs = support::queues(SCOPE);
const POLL_INTERVAL: Duration = Duration::from_millis(200);
const POLL_ATTEMPTS: usize = 80;
const NO_SIDE_EFFECT_OBSERVATION: Duration = Duration::from_secs(2);
const UNSAFE_IMAGE_URL: &str = "https://unsafe.shop.example/image.jpg";

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_deliver_committed_notification_delivery_and_persist_result() {
    let result = deliver_committed_notification_delivery().await;

    assert!(
        result.is_ok(),
        "notification delivery insert acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_retry_successful_delivery_finalization_without_sending_again() {
    let result = retry_successful_delivery_finalization().await;

    assert!(
        result.is_ok(),
        "notification delivery finalization retry acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_deliver_rolled_back_or_unsupported_cdc_changes() {
    let result = reject_rolled_back_or_unsupported_changes().await;

    assert!(
        result.is_ok(),
        "notification delivery rollback/filter acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_not_send_again_when_notification_delivery_is_redelivered() {
    let result = deduplicate_redelivered_notification_delivery().await;

    assert!(
        result.is_ok(),
        "notification delivery duplicate acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_deliver_after_expired_lease_is_redelivered() {
    let result = redeliver_after_expired_lease().await;

    assert!(
        result.is_ok(),
        "notification delivery lease/redelivery acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_persist_permanent_failure_when_template_is_invalid() {
    let result = persist_permanent_template_failure().await;

    assert!(
        result.is_ok(),
        "notification delivery failure acceptance test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_clear_retry_failure_state_when_a_retry_succeeds() {
    let result = clear_retry_failure_state_after_successful_delivery().await;

    assert!(
        result.is_ok(),
        "notification delivery retry-success acceptance test failed: {result:?}"
    );
}

async fn deliver_committed_notification_delivery() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let delivery =
            insert_delivery_with_language(&worker.pool, DeliveryState::Pending, "zh").await?;

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        assert_eq!(1, persisted.attempt_count);
        assert!(persisted.provider_message_id.is_some());
        assert!(persisted.delivered_at.is_some());
        assert!(persisted.last_error_code.is_none());
        assert!(persisted.lease_token.is_none());
        assert!(persisted.lease_expires_at.is_none());

        let email = wait_for_email_to(&delivery.recipient_email).await?;
        assert_eq!(
            vec![delivery.recipient_email],
            email.destination.to_addresses
        );
        assert_eq!("Your watchlist item's availability changed", email.subject);
        assert!(email.body.html_part.as_deref().is_some_and(|body| {
            body.contains("data-template-language=\"en\"")
                && body.contains("Delivery test source")
                && body.contains(
                    "https://aura-historia.com/product-listings/worker-delivery-product-abcdef",
                )
                && body.contains("Available")
                && body.contains("In stock")
        }));
        assert!(
            email
                .body
                .html_part
                .as_deref()
                .is_some_and(|body| !body.contains(UNSAFE_IMAGE_URL))
        );
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn retry_successful_delivery_finalization() -> Result<(), Box<dyn std::error::Error>> {
    let pool = get_postgres_client().await;
    let fault = Arc::new(FinalizationFaultState::default());
    fault.failures_remaining.store(1, Ordering::SeqCst);
    let worker = NotificationDeliveryWorker::start_with_repository(
        Template::Valid,
        pool.clone(),
        FailOnceFinalizationRepository {
            inner: SqlxNotificationDeliveryRepository::new(pool.clone()),
            state: fault.clone(),
        },
    )
    .await?;
    let result = async {
        let delivery = insert_delivery(&worker.pool, DeliveryState::Pending).await?;
        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;

        assert_eq!(1, persisted.attempt_count);
        assert!(persisted.provider_message_id.is_some());
        assert!(persisted.lease_token.is_none());
        assert!(persisted.lease_expires_at.is_none());
        let _ = wait_for_email_to(&delivery.recipient_email).await?;
        assert_email_count_for(&delivery.recipient_email, 1).await?;

        let calls = fault
            .calls
            .lock()
            .map_err(|_| std::io::Error::other("finalization fault state lock poisoned"))?
            .clone();
        assert_eq!(2, calls.len());
        assert_eq!(calls[0], calls[1]);
        assert_eq!(
            Some(calls[0].provider_message_id.clone()),
            persisted.provider_message_id
        );
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn reject_rolled_back_or_unsupported_changes() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let mut transaction = worker.pool.begin().await?;
        let rolled_back =
            insert_delivery_in_transaction(&mut transaction, DeliveryState::Pending).await?;
        drop(transaction);

        tokio::time::sleep(NO_SIDE_EFFECT_OBSERVATION).await;
        assert!(
            delivery_row(&worker.pool, rolled_back.delivery_id)
                .await?
                .is_none()
        );
        assert_no_email_to(&rolled_back.recipient_email).await?;

        let wrong_table = post_cdc_change(
            rolled_back.delivery_id,
            rolled_back.notification_id,
            "notifications",
            "insert",
        )
        .await
        .expect_err("unrouted notification CDC must be rejected by the active worker");
        assert!(wrong_table.to_string().contains("503"));

        let wrong_operation = post_cdc_change(
            rolled_back.delivery_id,
            rolled_back.notification_id,
            "notification_deliveries",
            "update",
        )
        .await
        .expect_err("unsupported notification CDC must be rejected by the active worker");
        assert!(wrong_operation.to_string().contains("503"));
        assert_no_email_to(&rolled_back.recipient_email).await
    }
    .await;

    worker.finish(result).await
}

async fn deduplicate_redelivered_notification_delivery() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let delivery = insert_delivery(&worker.pool, DeliveryState::Pending).await?;
        let initial = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        let initial_message_id = initial
            .provider_message_id
            .clone()
            .ok_or_else(|| std::io::Error::other("delivered row has no provider message id"))?;
        let _ = wait_for_email_to(&delivery.recipient_email).await?;

        post_cdc_change(
            delivery.delivery_id,
            delivery.notification_id,
            "notification_deliveries",
            "insert",
        )
        .await?;

        tokio::time::sleep(NO_SIDE_EFFECT_OBSERVATION).await;
        let persisted = delivery_row(&worker.pool, delivery.delivery_id)
            .await?
            .ok_or_else(|| std::io::Error::other("delivered row disappeared"))?;
        assert_eq!("DELIVERED", persisted.status);
        assert_eq!(1, persisted.attempt_count);
        assert_eq!(Some(initial_message_id), persisted.provider_message_id);
        assert_email_count_for(&delivery.recipient_email, 1).await
    }
    .await;

    worker.finish(result).await
}

async fn redeliver_after_expired_lease() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let delivery = insert_delivery(&worker.pool, DeliveryState::ActiveLease).await?;

        tokio::time::sleep(NO_SIDE_EFFECT_OBSERVATION).await;
        let claimed = delivery_row(&worker.pool, delivery.delivery_id)
            .await?
            .ok_or_else(|| std::io::Error::other("leased delivery row disappeared"))?;
        assert_eq!("PROCESSING", claimed.status);
        assert_eq!(4, claimed.attempt_count);
        assert!(claimed.lease_token.is_some());
        assert!(claimed.lease_expires_at.is_some());
        assert_no_email_to(&delivery.recipient_email).await?;

        sqlx::query(
            "UPDATE notification_deliveries SET lease_expires_at = now() - interval '1 second' WHERE notification_delivery_id = $1",
        )
        .bind(delivery.delivery_id.as_uuid())
        .execute(&worker.pool)
        .await?;

        post_cdc_change(
            delivery.delivery_id,
            delivery.notification_id,
            "notification_deliveries",
            "insert",
        )
        .await?;

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        assert_eq!(5, persisted.attempt_count);
        assert!(persisted.provider_message_id.is_some());
        assert_email_count_for(&delivery.recipient_email, 1).await
    }
    .await;

    worker.finish(result).await
}

async fn clear_retry_failure_state_after_successful_delivery()
-> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
    let result = async {
        let delivery =
            insert_delivery(&worker.pool, DeliveryState::PendingWithRetryableFailure).await?;

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
        assert_eq!(1, persisted.attempt_count);
        assert!(persisted.provider_message_id.is_some());
        assert!(persisted.delivered_at.is_some());
        assert!(persisted.last_error_code.is_none());
        assert!(persisted.lease_token.is_none());
        assert!(persisted.lease_expires_at.is_none());
        Ok(())
    }
    .await;

    worker.finish(result).await
}

async fn persist_permanent_template_failure() -> Result<(), Box<dyn std::error::Error>> {
    let worker = NotificationDeliveryWorker::start(Template::InvalidUtf8).await?;
    let result = async {
        let delivery = insert_delivery(&worker.pool, DeliveryState::Pending).await?;

        let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "FAILED").await?;
        assert_eq!(1, persisted.attempt_count);
        assert_eq!(
            Some("S3_TEMPLATE_INVALID_UTF8".to_owned()),
            persisted.last_error_code
        );
        assert!(persisted.provider_message_id.is_none());
        assert!(persisted.delivered_at.is_none());
        assert!(persisted.lease_token.is_none());
        assert!(persisted.lease_expires_at.is_none());
        assert_no_email_to(&delivery.recipient_email).await
    }
    .await;

    worker.finish(result).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeliveredFinalizationCall {
    lease_token: uuid::Uuid,
    provider_message_id: String,
    completed_at: time::OffsetDateTime,
}

#[derive(Default)]
struct FinalizationFaultState {
    failures_remaining: AtomicUsize,
    calls: Mutex<Vec<DeliveredFinalizationCall>>,
}

struct FailOnceFinalizationRepository {
    inner: SqlxNotificationDeliveryRepository,
    state: Arc<FinalizationFaultState>,
}

impl FailOnceFinalizationRepository {
    fn operation_error() -> NotificationDeliveryError {
        NotificationDeliveryError::OperationFailed {
            source: box_error(std::io::Error::other(
                "injected notification delivery finalization failure",
            )),
        }
    }

    fn should_fail_once(&self) -> bool {
        self.state
            .failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
    }
}

#[async_trait::async_trait]
impl NotificationDeliveryRepository for FailOnceFinalizationRepository {
    async fn claim_and_load_source(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        now: time::OffsetDateTime,
        lease_expires_at: time::OffsetDateTime,
        lease_token: uuid::Uuid,
    ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
        self.inner
            .claim_and_load_source(notification_delivery_id, now, lease_expires_at, lease_token)
            .await
    }

    async fn mark_delivered(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: uuid::Uuid,
        provider_message_id: &str,
        delivered_at: time::OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        self.state
            .calls
            .lock()
            .map_err(|_| Self::operation_error())?
            .push(DeliveredFinalizationCall {
                lease_token,
                provider_message_id: provider_message_id.to_owned(),
                completed_at: delivered_at,
            });
        if self.should_fail_once() {
            return Err(Self::operation_error());
        }
        self.inner
            .mark_delivered(
                notification_delivery_id,
                lease_token,
                provider_message_id,
                delivered_at,
            )
            .await
    }

    async fn mark_retryable_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: uuid::Uuid,
        error_code: &str,
        completed_at: time::OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        self.inner
            .mark_retryable_failure(
                notification_delivery_id,
                lease_token,
                error_code,
                completed_at,
            )
            .await
    }

    async fn mark_permanent_failure(
        &self,
        notification_delivery_id: NotificationDeliveryId,
        lease_token: uuid::Uuid,
        error_code: &str,
        completed_at: time::OffsetDateTime,
    ) -> Result<bool, NotificationDeliveryError> {
        self.inner
            .mark_permanent_failure(
                notification_delivery_id,
                lease_token,
                error_code,
                completed_at,
            )
            .await
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_preserve_provider_receipts_after_reversed_duplicate_sqs_deliveries() {
    let result: support::TestResult = async {
        let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
        let result = async {
            let first = insert_delivery(&worker.pool, DeliveryState::Pending).await?;
            let second = insert_delivery(&worker.pool, DeliveryState::Pending).await?;
            let first_receipt = wait_for_delivery(&worker.pool, first.delivery_id, "DELIVERED").await?.provider_message_id;
            let second_receipt = wait_for_delivery(&worker.pool, second.delivery_id, "DELIVERED").await?.provider_message_id;
            for delivery in [&second, &first, &second, &first] {
                post_cdc_change(
                    delivery.delivery_id,
                    delivery.notification_id,
                    "notification_deliveries",
                    "insert",
                )
                .await?;
            }
            support::wait_until_empty(SCOPE).await?;
            for (delivery, receipt) in [(&first, first_receipt), (&second, second_receipt)] {
                let persisted = delivery_row(&worker.pool, delivery.delivery_id).await?.ok_or("missing delivery")?;
                assert_eq!("DELIVERED", persisted.status);
                assert_eq!(1, persisted.attempt_count);
                assert_eq!(receipt, persisted.provider_message_id);
                assert_email_count_for(&delivery.recipient_email, 1).await?;
                let completion: (Option<uuid::Uuid>, Option<time::OffsetDateTime>) = sqlx::query_as("SELECT completed_lease_token, completed_at FROM notification_deliveries WHERE notification_delivery_id = $1")
                    .bind(delivery.delivery_id.as_uuid()).fetch_one(&worker.pool).await?;
                assert!(completion.0.is_some());
                assert!(completion.1.is_some());
            }
            Ok(())
        }.await;
        worker.finish(result).await
    }.await;
    result.expect("reversed SQS delivery acceptance and cleanup");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, WORKER_ACCEPTANCE, S3(), Ses(), WORKER_SQS, WORKER_SEQUIN])]
async fn should_retain_sqs_work_while_database_lease_is_active_then_deliver_without_republication()
{
    use aws_sdk_sqs::types::QueueAttributeName as A;
    let result: support::TestResult = async {
        let worker = NotificationDeliveryWorker::start(Template::Valid).await?;
        let result = async {
            let mut tx = worker.pool.begin().await?;
            let delivery = insert_delivery_in_transaction(&mut tx, DeliveryState::ActiveLease).await?;
            let expires: time::OffsetDateTime = sqlx::query_scalar("UPDATE notification_deliveries SET lease_expires_at = now() + interval '8 seconds' WHERE notification_delivery_id = $1 RETURNING lease_expires_at")
                .bind(delivery.delivery_id.as_uuid()).fetch_one(&mut *tx).await?;
            tx.commit().await?;
            // Only Sequin publishes. The queue must keep the message, not acknowledge AlreadyClaimed.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let attrs = test_api::get_sqs_client().await.get_queue_attributes().queue_url(WORKER_SQS.queue_url())
                    .attribute_names(A::ApproximateNumberOfMessagesNotVisible).send().await?;
                if attrs.attributes().and_then(|a| a.get(&A::ApproximateNumberOfMessagesNotVisible)).is_some_and(|n| n == "1") { break; }
                assert!(tokio::time::Instant::now() < deadline, "Sequin work was not retained under active DB lease");
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            assert!(time::OffsetDateTime::now_utc() < expires);
            let claimed = delivery_row(&worker.pool, delivery.delivery_id).await?.ok_or("missing active lease")?;
            assert_eq!("PROCESSING", claimed.status);
            assert_eq!(4, claimed.attempt_count);
            assert_eq!(Some(expires), claimed.lease_expires_at);
            assert_no_email_to(&delivery.recipient_email).await?;
            let persisted = wait_for_delivery(&worker.pool, delivery.delivery_id, "DELIVERED").await?;
            assert_eq!(5, persisted.attempt_count);
            assert!(persisted.lease_token.is_none());
            assert!(persisted.delivered_at.is_some_and(|at| at >= expires));
            assert_email_count_for(&delivery.recipient_email, 1).await?;
            support::wait_until_empty(SCOPE).await
        }.await;
        worker.finish(result).await
    }.await;
    result.expect("active lease SQS lifecycle acceptance and cleanup");
}

struct NotificationDeliveryWorker {
    pool: sqlx::PgPool,
    consumer: JoinHandle<()>,
    shutdown_tx: oneshot::Sender<()>,
    server: JoinHandle<Result<(), WorkerRunError>>,
    _route_lease: support::sequin_router::RouteLease,
}

impl NotificationDeliveryWorker {
    async fn start(template: Template) -> Result<Self, Box<dyn std::error::Error>> {
        let pool = get_postgres_client().await;
        Self::start_with_repository(
            template,
            pool.clone(),
            SqlxNotificationDeliveryRepository::new(pool),
        )
        .await
    }

    async fn start_with_repository<R>(
        template: Template,
        pool: sqlx::PgPool,
        repository: R,
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        R: NotificationDeliveryRepository + 'static,
    {
        let targets = DeliveryTargets::create(template).await?;
        let aws_config = test_api::localstack::get_aws_config().await;
        let s3 = S3Client::from_conf(
            S3ConfigBuilder::from(aws_config)
                .force_path_style(true)
                .build(),
        );
        let handler: Arc<dyn DeliverNotificationUseCase> =
            Arc::new(DeliverNotificationHandler::new(
                repository,
                NotificationDeliveryDispatcher::new(vec![Arc::new(
                    SesNotificationChannelSender::new(
                        s3,
                        SesClient::new(aws_config),
                        EmailDeliveryConfig::new(
                            targets.bucket,
                            "no-reply@notify.aura-historia.test",
                            "contact@aura-historia.test",
                            targets.stage,
                            targets.commit_sha,
                        ),
                        Arc::new(SqlxEmailDeliveryTargetReader::new(pool.clone())),
                    ),
                )
                    as Arc<dyn NotificationChannelSender>])?,
            ));
        let (runtime, receiver) = support::composition(SCOPE).await?.into_parts();
        let consumer = support::competing_consumers(SCOPE, receiver, move |receiver| {
            consume_notification_delivery_queue(receiver, handler.clone())
        })
        .await?;
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let local_addr = listener.local_addr()?;
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = tokio::spawn(serve_with_runtime(listener, runtime, async move {
            let _ = shutdown_rx.await;
        }));
        let route_lease = support::sequin_router::activate_scope(SCOPE, local_addr)
            .await
            .map_err(std::io::Error::other)?;

        Ok(Self {
            pool,
            consumer,
            shutdown_tx,
            server,
            _route_lease: route_lease,
        })
    }

    async fn finish(
        self,
        result: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let shutdown = self.shutdown().await;
        result?;
        shutdown
    }

    async fn shutdown(self) -> Result<(), Box<dyn std::error::Error>> {
        self.shutdown_tx
            .send(())
            .map_err(|_| std::io::Error::other("worker server shutdown channel closed"))?;
        self.server.await??;
        self.consumer.await?;
        Ok(())
    }
}

struct DeliveryTargets {
    bucket: String,
    stage: String,
    commit_sha: String,
}

impl DeliveryTargets {
    async fn create(template: Template) -> Result<Self, Box<dyn std::error::Error>> {
        let aws_config = test_api::localstack::get_aws_config().await;
        let s3 = S3Client::from_conf(
            S3ConfigBuilder::from(aws_config)
                .force_path_style(true)
                .build(),
        );
        let bucket = format!("worker-delivery-{}", uuid::Uuid::new_v4());
        let stage = "test".to_owned();
        let commit_sha = uuid::Uuid::new_v4().simple().to_string();
        s3.create_bucket()
            .bucket(&bucket)
            .create_bucket_configuration(
                CreateBucketConfiguration::builder()
                    .location_constraint(BucketLocationConstraint::EuCentral1)
                    .build(),
            )
            .send()
            .await?;
        s3.put_object()
            .bucket(&bucket)
            .key(format!(
                "{stage}/{commit_sha}/mjml/watchlist/product-update/availability/en.html"
            ))
            .body(template.bytes().into())
            .send()
            .await?;
        Ok(Self {
            bucket,
            stage,
            commit_sha,
        })
    }
}

#[derive(Clone, Copy)]
enum Template {
    Valid,
    InvalidUtf8,
}

impl Template {
    fn bytes(self) -> Vec<u8> {
        match self {
            Self::Valid => {
                b"<html><body data-template-language=\"en\">{{listing_source_name}} {{first_name}} {{old_availability}} {{new_availability}} <img src=\"{{image_url}}\"><a href=\"{{product_listing_url}}\">Listing</a><a href=\"{{view_url}}\">View</a></body></html>"
                    .to_vec()
            }
            Self::InvalidUtf8 => vec![0xff],
        }
    }
}

#[derive(Clone, Copy)]
enum DeliveryState {
    Pending,
    PendingWithRetryableFailure,
    ActiveLease,
}

struct NotificationDeliveryFixture {
    delivery_id: NotificationDeliveryId,
    notification_id: NotificationId,
    recipient_email: String,
}

async fn insert_delivery(
    pool: &sqlx::PgPool,
    state: DeliveryState,
) -> Result<NotificationDeliveryFixture, sqlx::Error> {
    insert_delivery_with_language(pool, state, "en").await
}

async fn insert_delivery_with_language(
    pool: &sqlx::PgPool,
    state: DeliveryState,
    language: &str,
) -> Result<NotificationDeliveryFixture, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    let delivery =
        insert_delivery_in_transaction_with_language(&mut transaction, state, language).await?;
    transaction.commit().await?;
    Ok(delivery)
}

async fn insert_delivery_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: DeliveryState,
) -> Result<NotificationDeliveryFixture, sqlx::Error> {
    insert_delivery_in_transaction_with_language(transaction, state, "en").await
}

async fn insert_delivery_in_transaction_with_language(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: DeliveryState,
    language: &str,
) -> Result<NotificationDeliveryFixture, sqlx::Error> {
    let user_id = UserId::new();
    let notification_id = NotificationId::new();
    let delivery_id = NotificationDeliveryId::new();
    let origin_event_id = EventId::new();
    let product_listing_id = ProductListingId::new();
    let recipient_email = format!("notification-delivery-{delivery_id}@example.test");

    sqlx::query(
        "INSERT INTO users (user_id, email, language, show_unassessed_or_sensitive_content, tier, role) VALUES ($1, $2, $3, false, 'ULTIMATE', 'USER')",
    )
    .bind(user_id.as_uuid())
    .bind(&recipient_email)
    .bind(language)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO notifications (notification_id, user_id, kind, origin_event_id, product_listing_id, payload_version, payload, seen) VALUES ($1, $2, 'WATCHLIST_AVAILABILITY_CHANGED', $3, $4, 1, $5, false)",
    )
    .bind(notification_id.as_uuid())
    .bind(user_id.as_uuid())
    .bind(origin_event_id.as_uuid())
    .bind(product_listing_id.as_uuid())
    .bind(notification_payload())
    .execute(&mut **transaction)
    .await?;

    match state {
        DeliveryState::Pending => {
            sqlx::query(
                "INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key) VALUES ($1, $2, 'EMAIL', 'PRIMARY')",
            )
            .bind(delivery_id.as_uuid())
            .bind(notification_id.as_uuid())
            .execute(&mut **transaction)
            .await?;
        }
        DeliveryState::PendingWithRetryableFailure => {
            sqlx::query(
                "INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key, last_error_code) VALUES ($1, $2, 'EMAIL', 'PRIMARY', 'S3_TEMPLATE_FETCH_RETRYABLE')",
            )
            .bind(delivery_id.as_uuid())
            .bind(notification_id.as_uuid())
            .execute(&mut **transaction)
            .await?;
        }
        DeliveryState::ActiveLease => {
            sqlx::query(
                "INSERT INTO notification_deliveries (notification_delivery_id, notification_id, channel, target_key, status, attempt_count, lease_token, lease_expires_at) VALUES ($1, $2, 'EMAIL', 'PRIMARY', 'PROCESSING', 4, $3, now() + interval '1 hour')",
            )
            .bind(delivery_id.as_uuid())
            .bind(notification_id.as_uuid())
            .bind(uuid::Uuid::new_v4())
            .execute(&mut **transaction)
            .await?;
        }
    }

    Ok(NotificationDeliveryFixture {
        delivery_id,
        notification_id,
        recipient_email,
    })
}

fn notification_payload() -> serde_json::Value {
    let listing_source_id = ListingSourceId::new();

    json!({
        "type": "WATCHLIST",
        "snapshot": {
            "listing_source_id": listing_source_id.as_uuid().to_string(),
            "source_listing_id": "worker-notification-delivery-product",
            "listing_source_slug_id": "worker-delivery-source",
            "product_listing_title_slug_id": "worker-delivery-product-abcdef",
            "listing_source_name": "Delivery test source",
            "title": null,
            "image": UNSAFE_IMAGE_URL,
            "content_policy": null,
            "url": "https://example.test/product_listings/delivery",
            "view_url": "https://aura-historia.com/product-listings/worker-delivery-product-abcdef"
        },
        "change": {
            "type": "AVAILABILITY_CHANGE",
            "old_availability": "AVAILABLE",
            "new_availability": "IN_STOCK"
        }
    })
}

#[derive(sqlx::FromRow)]
struct DeliveryRow {
    status: String,
    attempt_count: i32,
    lease_token: Option<uuid::Uuid>,
    lease_expires_at: Option<time::OffsetDateTime>,
    provider_message_id: Option<String>,
    last_error_code: Option<String>,
    delivered_at: Option<time::OffsetDateTime>,
}

async fn wait_for_delivery(
    pool: &sqlx::PgPool,
    delivery_id: NotificationDeliveryId,
    expected_status: &str,
) -> Result<DeliveryRow, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        if let Some(row) = delivery_row(pool, delivery_id).await?
            && row.status == expected_status
        {
            return Ok(row);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!(
        "notification delivery {delivery_id} did not reach {expected_status}"
    ))
    .into())
}

async fn delivery_row(
    pool: &sqlx::PgPool,
    delivery_id: NotificationDeliveryId,
) -> Result<Option<DeliveryRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT status, attempt_count, lease_token, lease_expires_at, provider_message_id, last_error_code, delivered_at FROM notification_deliveries WHERE notification_delivery_id = $1",
    )
    .bind(delivery_id.as_uuid())
    .fetch_optional(pool)
    .await
}

async fn post_cdc_change(
    delivery_id: NotificationDeliveryId,
    notification_id: NotificationId,
    table_name: &str,
    action: &str,
) -> support::TestResult {
    support::post_change(json!({
        "record": {
            "notification_delivery_id": delivery_id.as_uuid().to_string(),
            "notification_id": notification_id.as_uuid().to_string(),
            "channel": "EMAIL",
            "status": "PENDING"
        },
        "action": action,
        "metadata": {"table_schema": "public", "table_name": table_name}
    }))
    .await
}

async fn wait_for_email_to(
    recipient_email: &str,
) -> Result<test_api::SentEmail, Box<dyn std::error::Error>> {
    for _ in 0..POLL_ATTEMPTS {
        if let Some(email) = get_sent_emails().await.into_iter().find(|email| {
            email
                .destination
                .to_addresses
                .iter()
                .any(|address| address == recipient_email)
        }) {
            return Ok(email);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(std::io::Error::other(format!("no SES email sent to {recipient_email}")).into())
}

async fn assert_no_email_to(recipient_email: &str) -> Result<(), Box<dyn std::error::Error>> {
    if get_sent_emails().await.into_iter().any(|email| {
        email
            .destination
            .to_addresses
            .iter()
            .any(|address| address == recipient_email)
    }) {
        return Err(std::io::Error::other(format!(
            "unexpected SES email sent to {recipient_email}"
        ))
        .into());
    }
    Ok(())
}

async fn assert_email_count_for(
    recipient_email: &str,
    expected_count: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let actual_count = get_sent_emails()
        .await
        .into_iter()
        .filter(|email| {
            email
                .destination
                .to_addresses
                .iter()
                .any(|address| address == recipient_email)
        })
        .count();
    if actual_count != expected_count {
        return Err(std::io::Error::other(format!(
            "expected {expected_count} SES emails to {recipient_email}, found {actual_count}"
        ))
        .into());
    }
    Ok(())
}

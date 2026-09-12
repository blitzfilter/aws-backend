mod preflight;
mod shutdown;
#[cfg(test)]
mod shutdown_tests;

use aura_historia_worker::notification_delivery::consume_notification_delivery_queue;
use aura_historia_worker::product_content_assessment::consume_product_content_assessment_queue;
use aura_historia_worker::product_embedding::consume_product_embedding_queue;
use aura_historia_worker::product_listing_opensearch::consume_product_listing_opensearch_queue;
use aura_historia_worker::product_listing_raw_normalization::consume_product_listing_raw_normalization_queue;
use aura_historia_worker::product_translation::consume_product_translation_queue;
use aura_historia_worker::search_filter_match_notifications::consume_search_filter_match_notification_queue;
use aura_historia_worker::search_filter_percolator::consume_search_filter_percolator_queue;
use aura_historia_worker::search_filter_projection::consume_search_filter_projection_queue;
use aura_historia_worker::watchlist_notifications::consume_watchlist_notification_queue;
use aura_historia_worker::{
    WorkerOpenSearchConfig, WorkerRunError, WorkerRuntimeComposition, WorkerScope,
    WorkerStartupConfig, WorkerStartupConfigError, WorkerVertexAiConfig,
    run_until_shutdown_with_runtime,
};
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::Client as SesClient;
use aws_smithy_types::timeout::TimeoutConfig;
use embedding::{VertexAiEmbeddingConfig, VertexAiEmbeddingGenerator};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use google_cloud_auth::credentials::Builder as GoogleCredentialsBuilder;
use large_language_model::{VertexAiConfig, VertexAiGemini};
use notification_core::notification_delivery::NotificationDeliveryChannel;
use notification_email_aws::{EmailDeliveryConfig, SesNotificationChannelSender};
use notification_postgres::{
    SqlxEmailDeliveryTargetReader, SqlxNotificationDeliveryIntentRepositoryFactory,
    SqlxNotificationDeliveryRepository, SqlxNotificationRepositoryFactory,
};
use notification_service::{
    initial_external_delivery_plan_reader::InitialExternalDeliveryPlanReaderFactory,
    notification_creation::NotificationCreationCoordinatorFactory,
};
use notification_service::{
    ports::notification_channel_sender::{
        NotificationChannelSender, NotificationDeliveryDispatcher,
    },
    use_cases::commands::deliver_notification::{
        DeliverNotificationHandler, DeliverNotificationUseCase,
    },
};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_observability::{LogLevel, LoggingConfig, init};
use platform_postgres::{PostgresConnectError, PostgresSchemaError, SqlxUnitOfWork};
use product_listing_opensearch::OpenSearchProductListingSearchProjection;
use product_listing_postgres::{
    SqlxPendingProductListingRawStreamReader,
    SqlxProductListingContentAssessmentSnapshotReaderFactory,
    SqlxProductListingContentAssessmentSourceReader,
    SqlxProductListingContentAssessmentWriterFactory, SqlxProductListingCurrentEventGuardFactory,
    SqlxProductListingEmbeddingSourceReader, SqlxProductListingEmbeddingWriterFactory,
    SqlxProductListingEventAppenderFactory, SqlxProductListingRawNormalizationWriterFactory,
    SqlxProductListingRepositoryFactory, SqlxProductListingSearchFilterMatchSourceReaderFactory,
    SqlxProductListingTranslationSourceReader, SqlxProductListingTranslationWriterFactory,
    SqlxProductListingWatchlistNotificationSourceReaderFactory,
};
use product_listing_service::use_cases::{
    AssessProductListingContentEventHandler, AssessProductListingContentEventUseCase,
    EmbedProductListingEventHandler, EmbedProductListingEventUseCase,
    GenerateWatchlistNotificationsHandler, GenerateWatchlistNotificationsUseCase,
    ProjectProductListingHandler, ProjectProductListingUseCase,
    TranslateProductListingEventHandler, TranslateProductListingEventUseCase,
};
use product_listing_translation_llm::LargeLanguageModelProductListingTitleTranslator;
use product_service::use_cases::{
    NormalizeProductListingRawRevisionHandler, NormalizeProductListingRawRevisionUseCase,
};
use search_filter_opensearch::OpenSearchSearchFilterIndex;
use search_filter_postgres::{
    SqlxActiveSearchFilterMatchCandidateReaderFactory, SqlxSearchFilterIndexReader,
    SqlxSearchFilterMatchNotificationSourceReaderFactory, SqlxSearchFilterMatchWriterFactory,
    SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
};
use search_filter_service::use_cases::{
    GenerateSearchFilterMatchNotificationHandler, GenerateSearchFilterMatchNotificationUseCase,
    MatchProductListingEventHandler, MatchProductListingEventUseCase,
    ProjectSearchFilterChangeHandler, ProjectSearchFilterChangeUseCase,
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::watch;
use user_postgres::SqlxUserTierEntitlementsFactory;
use watchlist_postgres::SqlxWatchlistNotificationRecipientReaderFactory;

const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

fn main() -> std::process::ExitCode {
    std::panic::set_hook(Box::new(|_| {
        tracing::error!(outcome = "worker_panic", "worker task panicked")
    }));
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            tracing::error!(category = "RUNTIME_INITIALIZATION", "worker runtime failed");
            return std::process::ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run());
    shutdown::teardown(runtime);
    if let Err(error) = result {
        // Provider SDK/error bodies can contain secrets. Do not print MainError's source chain.
        tracing::error!(
            outcome = "worker_stopped",
            category = error.category(),
            "worker startup or supervision failed"
        );
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

async fn run() -> Result<(), MainError> {
    // Both registrations are synchronous, before configuration, connection or AWS setup.
    let mut signals = ShutdownSignals::register()?;
    init(LoggingConfig::new(
        std::env::var("LOG_LEVEL")
            .ok()
            .as_deref()
            .and_then(LogLevel::parse)
            .unwrap_or_default(),
    ));
    let (stop, stopped) = watch::channel(false);
    let work = run_with_shutdown(stopped);
    tokio::pin!(work);
    tokio::select! {
        biased;
        () = signals.wait() => {
            tracing::info!(outcome = "worker_draining", "worker shutdown requested");
            stop.send_replace(true);
            work.await
        }
        result = &mut work => result,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Worker,
    CheckConfig,
}
impl RunMode {
    fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self, MainError> {
        let mut args = args.into_iter();
        match (args.next(), args.next()) {
            (None, None) => Ok(Self::Worker),
            (Some(arg), None) if arg == "--check-config" => Ok(Self::CheckConfig),
            _ => Err(MainError::Arguments),
        }
    }
}

struct RuntimeConfig {
    worker: aura_historia_worker::WorkerConfig,
    stopped: watch::Receiver<bool>,
}

async fn run_with_shutdown(mut stopped: watch::Receiver<bool>) -> Result<(), MainError> {
    let mode = RunMode::parse(std::env::args_os().skip(1))?;
    let startup = WorkerStartupConfig::from_env()?;
    let prepared = tokio::select! {
        biased;
        _ = stopped.wait_for(|stop| *stop) => return if mode == RunMode::CheckConfig { Err(MainError::StartupInterrupted) } else { Ok(()) },
        result = preflight::check(&startup) => result?,
    };
    if mode == RunMode::CheckConfig {
        prepared.pool.close().await;
        tracing::info!(
            outcome = "worker_config_checked",
            "read-only worker preflight passed"
        );
        return Ok(());
    }
    if *stopped.borrow() {
        prepared.pool.close().await;
        return Ok(());
    }
    let scope = startup.scope();
    let worker_config = RuntimeConfig {
        worker: startup.worker().clone(),
        stopped,
    };
    let pool = prepared.pool;
    let composition = WorkerRuntimeComposition::from_sqs_queue(prepared.queue);

    match scope {
        WorkerScope::SearchFilterProjection => {
            let opensearch = startup
                .opensearch()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_search_filter_projection(worker_config, pool, composition, opensearch).await
        }
        WorkerScope::SearchFilterPercolator => {
            let opensearch = startup
                .opensearch()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            let vertex_ai = startup
                .vertex_ai()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_search_filter_percolator(worker_config, pool, composition, opensearch, vertex_ai)
                .await
        }
        WorkerScope::SearchFilterMatchNotification => {
            run_search_filter_match_notifications(worker_config, pool, composition).await
        }
        WorkerScope::WatchlistNotification => {
            run_watchlist_notifications(worker_config, pool, composition).await
        }
        WorkerScope::ProductListingContentAssessment => {
            run_product_content_assessment(worker_config, pool, composition).await
        }
        WorkerScope::ProductListingTranslation => {
            let vertex_ai = startup
                .vertex_ai()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_product_translation(worker_config, pool, composition, vertex_ai).await
        }
        WorkerScope::ProductListingOpenSearch => {
            let opensearch = startup
                .opensearch()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_product_listing_opensearch(worker_config, pool, composition, opensearch).await
        }
        WorkerScope::ProductListingEmbedding => {
            let vertex_ai = startup
                .vertex_ai()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_product_embedding(worker_config, pool, composition, vertex_ai).await
        }
        WorkerScope::ProductListingRawNormalization => {
            run_product_listing_raw_normalization(worker_config, pool, composition).await
        }
        WorkerScope::NotificationDelivery => {
            let delivery = startup
                .notification_delivery()
                .ok_or(MainError::MissingScopeConfig { scope })?;
            run_notification_delivery(worker_config, pool, composition, delivery).await
        }
    }
}

async fn run_search_filter_projection(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    opensearch: &WorkerOpenSearchConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn ProjectSearchFilterChangeUseCase> =
        Arc::new(ProjectSearchFilterChangeHandler::new(
            SqlxSearchFilterIndexReader::new(pool),
            OpenSearchSearchFilterIndex::new(opensearch_client(opensearch)?),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_search_filter_projection_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn run_search_filter_percolator(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    opensearch: &WorkerOpenSearchConfig,
    vertex_ai: &WorkerVertexAiConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn MatchProductListingEventUseCase> =
        Arc::new(MatchProductListingEventHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
            SqlxProductListingCurrentEventGuardFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
            OpenSearchSearchFilterIndex::new(opensearch_client(opensearch)?),
            vertex_ai_large_language_model(vertex_ai)?,
            SqlxActiveSearchFilterMatchCandidateReaderFactory,
            SqlxSearchFilterMatchWriterFactory,
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_search_filter_percolator_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn run_search_filter_match_notifications(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
) -> Result<(), MainError> {
    let handler: Arc<dyn GenerateSearchFilterMatchNotificationUseCase> =
        Arc::new(GenerateSearchFilterMatchNotificationHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxSearchFilterMatchNotificationSourceReaderFactory,
            SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
            SqlxSearchFilterMonthlyMatchQuotaReaderFactory,
            SqlxUserTierEntitlementsFactory::new(),
            SqlxProductListingContentAssessmentSnapshotReaderFactory::new(),
            NotificationCreationCoordinatorFactory::new(
                SqlxNotificationRepositoryFactory::new(),
                InitialExternalDeliveryPlanReaderFactory,
                SqlxNotificationDeliveryIntentRepositoryFactory::new(),
            ),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_search_filter_match_notification_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn run_product_listing_opensearch(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    opensearch: &WorkerOpenSearchConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn ProjectProductListingUseCase> =
        Arc::new(ProjectProductListingHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxProductListingSearchFilterMatchSourceReaderFactory::new(),
            SqlxFxRateSnapshotRepositoryFactory,
            OpenSearchProductListingSearchProjection::new(opensearch_client(opensearch)?),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_product_listing_opensearch_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn run_product_content_assessment(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
) -> Result<(), MainError> {
    let handler: Arc<dyn AssessProductListingContentEventUseCase> =
        Arc::new(AssessProductListingContentEventHandler::new(
            SqlxProductListingContentAssessmentSourceReader::new(pool.clone()),
            SqlxUnitOfWork::new(pool),
            SqlxProductListingContentAssessmentWriterFactory::new(),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_product_content_assessment_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn run_product_embedding(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    vertex_ai: &WorkerVertexAiConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn EmbedProductListingEventUseCase> =
        Arc::new(EmbedProductListingEventHandler::new(
            SqlxProductListingEmbeddingSourceReader::new(pool.clone()),
            VertexAiEmbeddingGenerator::new(
                VertexAiEmbeddingConfig::new(vertex_ai.project_id(), vertex_ai.location()),
                vertex_ai_credentials()?,
            ),
            SqlxUnitOfWork::new(pool),
            SqlxProductListingEmbeddingWriterFactory::new(),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_product_embedding_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn run_product_translation(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    vertex_ai: &WorkerVertexAiConfig,
) -> Result<(), MainError> {
    let handler: Arc<dyn TranslateProductListingEventUseCase> =
        Arc::new(TranslateProductListingEventHandler::new(
            SqlxProductListingTranslationSourceReader::new(pool.clone()),
            LargeLanguageModelProductListingTitleTranslator::new(vertex_ai_large_language_model(
                vertex_ai,
            )?),
            SqlxUnitOfWork::new(pool),
            SqlxProductListingTranslationWriterFactory::new(),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_product_translation_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn run_product_listing_raw_normalization(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
) -> Result<(), MainError> {
    let handler: Arc<dyn NormalizeProductListingRawRevisionUseCase> =
        Arc::new(NormalizeProductListingRawRevisionHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductListingRawNormalizationWriterFactory::new(),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPendingProductListingRawStreamReader::new(pool),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task =
        consume_product_listing_raw_normalization_queue(receiver, handler, config.stopped.clone());
    finish_runtime(config, runtime, task).await
}

async fn run_notification_delivery(
    mut config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
    delivery: &aura_historia_worker::WorkerNotificationDeliveryConfig,
) -> Result<(), MainError> {
    let email = delivery.email().ok_or(MainError::MissingScopeConfig {
        scope: WorkerScope::NotificationDelivery,
    })?;
    let aws_config = aws_config::defaults(BehaviorVersion::v2026_01_12())
        .timeout_config(
            TimeoutConfig::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .operation_attempt_timeout(std::time::Duration::from_secs(20))
                .operation_timeout(std::time::Duration::from_secs(30))
                .build(),
        )
        .load();
    let aws_config = tokio::select! {
        biased;
        _ = config.stopped.wait_for(|stop| *stop) => return Ok(()),
        result = tokio::time::timeout(Duration::from_secs(15), aws_config) => result.map_err(|_| MainError::AwsStartupTimeout)?,
    };
    let dispatcher =
        NotificationDeliveryDispatcher::new(vec![Arc::new(SesNotificationChannelSender::new(
            S3Client::new(&aws_config),
            SesClient::new(&aws_config),
            EmailDeliveryConfig::new(
                email.template_bucket(),
                email.from_email_address(),
                email.reply_to_email_address(),
                email.stage(),
                email.commit_sha(),
            ),
            Arc::new(SqlxEmailDeliveryTargetReader::new(pool.clone())),
        )) as Arc<dyn NotificationChannelSender>])
        .map_err(MainError::NotificationDispatcher)?;
    dispatcher
        .validate_channels([NotificationDeliveryChannel::Email])
        .map_err(MainError::NotificationDispatch)?;
    let handler: Arc<dyn DeliverNotificationUseCase> = Arc::new(DeliverNotificationHandler::new(
        SqlxNotificationDeliveryRepository::new(pool),
        dispatcher,
    ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_notification_delivery_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn run_watchlist_notifications(
    config: RuntimeConfig,
    pool: sqlx::PgPool,
    composition: WorkerRuntimeComposition,
) -> Result<(), MainError> {
    let handler: Arc<dyn GenerateWatchlistNotificationsUseCase> =
        Arc::new(GenerateWatchlistNotificationsHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductListingWatchlistNotificationSourceReaderFactory::new(),
            SqlxWatchlistNotificationRecipientReaderFactory,
            NotificationCreationCoordinatorFactory::new(
                SqlxNotificationRepositoryFactory::new(),
                InitialExternalDeliveryPlanReaderFactory,
                SqlxNotificationDeliveryIntentRepositoryFactory::new(),
            ),
        ));
    let (runtime, receiver) = composition.into_parts();
    let task = consume_watchlist_notification_queue(receiver, handler);
    finish_runtime(config, runtime, task).await
}

async fn finish_runtime(
    mut config: RuntimeConfig,
    runtime: aura_historia_worker::WorkerRuntime,
    consumer: impl Future<Output = ()> + Send + 'static,
) -> Result<(), MainError> {
    if *config.stopped.borrow() {
        runtime.shutdown();
        return Ok(());
    }
    let task = tokio::spawn(consumer);
    let drain_timeout = config.worker.drain_timeout();
    let (stop_http, stopped) = tokio::sync::oneshot::channel();
    let server = run_until_shutdown_with_runtime(config.worker, runtime.clone(), async move {
        let _closed = stopped.await;
    });
    supervise_runtime(
        runtime,
        task,
        server,
        async {
            let _closed = config.stopped.wait_for(|stop| *stop).await;
        },
        stop_http,
        drain_timeout,
    )
    .await
}

async fn supervise_runtime<S, G>(
    runtime: aura_historia_worker::WorkerRuntime,
    task: tokio::task::JoinHandle<()>,
    server: S,
    shutdown: G,
    stop_http: tokio::sync::oneshot::Sender<()>,
    drain_timeout: Duration,
) -> Result<(), MainError>
where
    S: Future<Output = Result<(), WorkerRunError>>,
    G: Future<Output = ()>,
{
    // Declared first, dropped last: cover both branches and their future destructors.
    let mut _shutdown_deadline = None;
    let mut consumer = SupervisedConsumer(task);
    let mut stop_http = Some(stop_http);
    tokio::pin!(server);
    tokio::pin!(shutdown);
    tokio::select! {
        biased;
        () = &mut shutdown => {
            _shutdown_deadline = Some(shutdown::FatalDeadline::for_runtime(drain_timeout));
            runtime.shutdown();
            if let Some(stop_http) = stop_http.take() {
                let _closed = stop_http.send(());
            }
            let (server_result, consumer_result) = tokio::join!(server, consumer.drain(&runtime, drain_timeout));
            consumer_result.and(server_result.map_err(MainError::from))
        }
        result = &mut consumer.0 => {
            _shutdown_deadline = Some(shutdown::FatalDeadline::for_runtime(drain_timeout));
            runtime.shutdown();
            if let Some(stop_http) = stop_http.take() {
                let _closed = stop_http.send(());
            }
            let (_server_result, ()) = tokio::join!(server, consumer.cleanup(&runtime, true));
            let _joined = result;
            Err(MainError::ConsumerStopped)
        }
        result = &mut server => {
            _shutdown_deadline = Some(shutdown::FatalDeadline::for_runtime(drain_timeout));
            runtime.shutdown();
            consumer.drain(&runtime, drain_timeout).await.and(result.map_err(MainError::from))
        }
    }
}

struct SupervisedConsumer(tokio::task::JoinHandle<()>);
impl Drop for SupervisedConsumer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
impl SupervisedConsumer {
    async fn drain(
        &mut self,
        runtime: &aura_historia_worker::WorkerRuntime,
        drain_timeout: Duration,
    ) -> Result<(), MainError> {
        // Tokio timers cannot enforce a deadline if cancellation blocks every worker thread.
        let _deadline = shutdown::FatalDeadline::after_drain(drain_timeout);
        let (result, joined) = match tokio::time::timeout(drain_timeout, &mut self.0).await {
            Ok(Ok(())) => (Ok(()), true),
            Ok(Err(_)) => (Err(MainError::ConsumerStopped), true),
            Err(_) => {
                tracing::error!(
                    outcome = "worker_drain_deadline",
                    drain_timeout_seconds = drain_timeout.as_secs(),
                    "consumer drain deadline exceeded; aborting local work"
                );
                self.0.abort();
                (
                    Err(MainError::ConsumerDrainDeadline {
                        seconds: drain_timeout.as_secs(),
                    }),
                    false,
                )
            }
        };
        self.cleanup(runtime, joined).await;
        result
    }

    async fn cleanup(&mut self, runtime: &aura_historia_worker::WorkerRuntime, joined: bool) {
        shutdown::join_cancelled(
            async {
                if !joined {
                    let _cancelled = (&mut self.0).await;
                }
            },
            runtime.join_cancelled_tasks(),
        )
        .await;
    }
}

fn vertex_ai_large_language_model(
    config: &WorkerVertexAiConfig,
) -> Result<VertexAiGemini, MainError> {
    let config = VertexAiConfig::new(
        config.project_id().to_owned(),
        config.location().to_owned(),
        config
            .model()
            .ok_or(MainError::MissingVertexAiModel)?
            .to_owned(),
    );
    let credentials = vertex_ai_credentials()?;
    VertexAiGemini::new(config, credentials).map_err(MainError::VertexAiHttpClient)
}

fn vertex_ai_credentials()
-> Result<google_cloud_auth::credentials::AccessTokenCredentials, MainError> {
    GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|error| MainError::VertexAiCredentials {
            detail: error.to_string(),
        })
}

fn opensearch_client(config: &WorkerOpenSearchConfig) -> Result<OpenSearch, MainError> {
    let pool = SingleNodeConnectionPool::new(config.endpoint().clone());
    let builder = TransportBuilder::new(pool);
    let transport = match config.basic_auth() {
        Some((username, password)) => {
            builder.auth(Credentials::Basic(username.to_owned(), password.to_owned()))
        }
        None => builder,
    }
    .build()
    .map_err(|error| MainError::OpenSearch {
        detail: error.to_string(),
    })?;
    Ok(OpenSearch::new(transport))
}

struct ShutdownSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}
impl ShutdownSignals {
    fn register() -> Result<Self, MainError> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt()).map_err(|_| MainError::SignalSetup)?,
            terminate: signal(SignalKind::terminate()).map_err(|_| MainError::SignalSetup)?,
        })
    }
    async fn wait(&mut self) {
        tokio::select! {
            _ = self.interrupt.recv() => {},
            _ = self.terminate.recv() => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_historia_worker::WorkerRuntime;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::sync::oneshot;

    const EMPTY_CDC_BATCH: &str = r#"{"changes":[]}"#;

    #[test]
    fn should_accept_only_worker_or_explicit_check_config_arguments() {
        for args in [vec![], vec!["--check-config"]] {
            assert!(RunMode::parse(args.into_iter().map(std::ffi::OsString::from)).is_ok());
        }
        for args in [
            vec!["--migrate"],
            vec!["--check-config", "extra"],
            vec!["--check-config=1"],
            vec!["secret-canary"],
        ] {
            let error = RunMode::parse(args.into_iter().map(std::ffi::OsString::from))
                .err()
                .unwrap();
            assert_eq!("ARGUMENTS_INVALID", error.category());
            assert!(!format!("{error:?} {error}").contains("secret-canary"));
        }
    }

    struct DropSignal(Arc<AtomicBool>);
    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    async fn assert_runtime_stops_ingress(runtime: &WorkerRuntime) {
        assert!(runtime.ingest_cdc_json(EMPTY_CDC_BATCH).await.is_err());
    }

    #[tokio::test]
    async fn should_stop_runtime_when_consumer_completes_or_panics_unexpectedly() {
        for panics in [false, true] {
            let runtime = WorkerRuntime::empty();
            assert!(runtime.ingest_cdc_json(EMPTY_CDC_BATCH).await.is_ok());
            let dropped = Arc::new(AtomicBool::new(false));
            let guard = DropSignal(dropped.clone());
            let task = tokio::spawn(async move {
                let _guard = guard;
                if panics {
                    panic!("test consumer panic");
                }
            });
            let (stop_http, stopped) = oneshot::channel();
            let server = async move {
                let _closed = stopped.await;
                Ok::<(), WorkerRunError>(())
            };

            let result = supervise_runtime(
                runtime.clone(),
                task,
                server,
                std::future::pending::<()>(),
                stop_http,
                Duration::from_secs(1),
            )
            .await;

            assert!(matches!(result, Err(MainError::ConsumerStopped)));
            assert!(dropped.load(Ordering::Acquire));
            assert_runtime_stops_ingress(&runtime).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn should_abort_and_join_consumer_when_drain_deadline_expires() {
        let runtime = WorkerRuntime::empty();
        assert!(runtime.ingest_cdc_json(EMPTY_CDC_BATCH).await.is_ok());
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = DropSignal(dropped.clone());
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        let (stop_http, stopped) = oneshot::channel();
        let server = async move {
            let _closed = stopped.await;
            Ok::<(), WorkerRunError>(())
        };

        let result = supervise_runtime(
            runtime.clone(),
            task,
            server,
            async {},
            stop_http,
            Duration::from_secs(1),
        )
        .await;

        assert!(matches!(
            result,
            Err(MainError::ConsumerDrainDeadline { seconds: 1 })
        ));
        assert!(dropped.load(Ordering::Acquire));
        assert_runtime_stops_ingress(&runtime).await;
    }

    #[tokio::test]
    async fn should_allow_active_consumer_to_finish_inside_drain_deadline() {
        let runtime = WorkerRuntime::empty();
        let (finish_consumer, consumer_finished) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _closed = consumer_finished.await;
        });
        let (stop_http, stopped) = oneshot::channel();
        let server = async move {
            let _closed = stopped.await;
            let _consumer_already_stopped = finish_consumer.send(());
            Ok::<(), WorkerRunError>(())
        };

        let result = supervise_runtime(
            runtime.clone(),
            task,
            server,
            async {},
            stop_http,
            Duration::from_secs(1),
        )
        .await;

        assert!(result.is_ok());
        assert_runtime_stops_ingress(&runtime).await;
    }
}

impl MainError {
    fn category(&self) -> &'static str {
        match self {
            Self::StartupConfig(_) => "CONFIG_INVALID",
            Self::Postgres(_) => "POSTGRES_UNAVAILABLE",
            Self::PostgresSchema(error) => error.code(),
            Self::QueueConfig(_) => "SQS_CONTRACT_OR_DEPENDENCY",
            Self::Arguments => "ARGUMENTS_INVALID",
            Self::SignalSetup => "SIGNAL_REGISTRATION",
            Self::StartupInterrupted => "PREFLIGHT_INTERRUPTED",
            Self::AwsStartupTimeout => "AWS_STARTUP_TIMEOUT",
            Self::OpenSearchCompatibility => "OPENSEARCH_COMPATIBILITY",

            Self::ConsumerDrainDeadline { .. } => "DRAIN_DEADLINE",
            Self::ConsumerStopped => "CONSUMER_STOPPED",
            Self::Run(_) => "HTTP_SERVER",
            _ => "SCOPED_ADAPTER_STARTUP",
        }
    }
}

#[derive(thiserror::Error, Debug)]
enum MainError {
    #[error(transparent)]
    StartupConfig(#[from] WorkerStartupConfigError),
    #[error(transparent)]
    Postgres(#[from] PostgresConnectError),
    #[error(transparent)]
    PostgresSchema(#[from] PostgresSchemaError),
    #[error("expected no arguments or --check-config")]
    Arguments,
    #[error("failed to register worker shutdown signals")]
    SignalSetup,
    #[error("worker preflight interrupted before verification completed")]
    StartupInterrupted,
    #[error("AWS startup timed out")]
    AwsStartupTimeout,
    #[error("OpenSearch read-only compatibility check failed")]
    OpenSearchCompatibility,

    #[error(transparent)]
    QueueConfig(#[from] aura_historia_worker::queue::QueueError),
    #[error("missing validated configuration for {scope:?} worker scope")]
    MissingScopeConfig { scope: WorkerScope },

    #[error("failed to configure OpenSearch: {detail}")]
    OpenSearch { detail: String },
    #[error("failed to initialize Vertex AI credentials: {detail}")]
    VertexAiCredentials { detail: String },
    #[error("failed to build Vertex AI HTTP client: {0}")]
    VertexAiHttpClient(reqwest::Error),
    #[error("validated Vertex AI LLM configuration is missing its model")]
    MissingVertexAiModel,
    #[error(transparent)]
    NotificationDispatcher(
        #[from] notification_service::ports::notification_channel_sender::NotificationDeliveryDispatcherRegistrationError,
    ),
    #[error(transparent)]
    NotificationDispatch(
        #[from] notification_service::ports::notification_channel_sender::NotificationDeliveryDispatchError,
    ),
    #[error("worker consumer stopped unexpectedly")]
    ConsumerStopped,
    #[error("worker consumer drain exceeded {seconds}s shutdown budget")]
    ConsumerDrainDeadline { seconds: u64 },
    #[error(transparent)]
    Run(#[from] WorkerRunError),
}

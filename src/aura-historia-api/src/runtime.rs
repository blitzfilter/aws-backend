use crate::state::{AppState, OperationsState, ReadinessCheck};
use crate::{ApiConfig, ApiRunError, ApiStateError, RuntimeReadiness, app_state_from_config};
use axum::{Router, extract::Request, middleware::Next, response::IntoResponse};
use hyper::server::conn::http1;
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use opensearch::{
    OpenSearch,
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, TransportBuilder},
};
use platform_postgres::{PostgresPoolConfig, verify_business_schema};
use sqlx::PgPool;
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::watch,
    task::JoinSet,
    time::{Instant, timeout, timeout_at},
};

const OPERATIONS_BIND: &str = "AURA_HISTORIA_API_OPERATIONS_BIND_ADDR";
const DRAIN_SECONDS: &str = "AURA_HISTORIA_API_DRAIN_SECONDS";
const STOP_SECONDS: &str = "AURA_HISTORIA_API_STOP_SECONDS";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(30);
const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(5);
const POOL_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const TASK_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);
const CLEANUP_RESERVE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ApiLifecycleState {
    Starting,
    Ready,
    Draining,
    Stopped,
}

impl ApiLifecycleState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "STARTING",
            Self::Ready => "READY",
            Self::Draining => "DRAINING",
            Self::Stopped => "STOPPED",
        }
    }
}

pub(crate) struct Lifecycle {
    state: AtomicU8,
    dependencies_ready: AtomicBool,
}

impl Lifecycle {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            dependencies_ready: AtomicBool::new(false),
        }
    }

    pub(crate) fn state(&self) -> ApiLifecycleState {
        match self.state.load(Ordering::SeqCst) {
            0 => ApiLifecycleState::Starting,
            1 => ApiLifecycleState::Ready,
            2 => ApiLifecycleState::Draining,
            // Fail closed; no unknown value can admit work.
            _ => ApiLifecycleState::Stopped,
        }
    }

    fn mark_ready(&self) {
        self.dependencies_ready.store(true, Ordering::SeqCst);
        let _previous = self
            .state
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
    }

    fn begin_drain(&self) {
        self.state.fetch_max(2, Ordering::SeqCst);
        self.dependencies_ready.store(false, Ordering::SeqCst);
    }

    fn mark_stopped(&self) {
        self.state.store(3, Ordering::SeqCst);
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.state() == ApiLifecycleState::Ready && self.dependencies_ready.load(Ordering::SeqCst)
    }
}

struct LifecycleConfig {
    operations_bind: SocketAddr,
    commit_sha: Arc<str>,
    drain: Duration,
}

impl LifecycleConfig {
    fn from_getter(
        get: &mut impl FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, ApiStateError> {
        let stage = required(get, "STAGE")?;
        let real = match stage.as_str() {
            "dev" | "prod" => true,
            "local" | "test" | "ephemeral" => false,
            _ => return Err(invalid("STAGE")),
        };
        let commit_sha = required(get, "COMMIT_SHA")?;
        let full_sha = commit_sha.len() == 40
            && commit_sha
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !full_sha && (real || commit_sha != "unversioned") {
            return Err(invalid("COMMIT_SHA"));
        }
        let operations_bind: SocketAddr = get(OPERATIONS_BIND)
            .unwrap_or_else(|| "127.0.0.1:9080".into())
            .parse()
            .map_err(|_| invalid(OPERATIONS_BIND))?;
        // No public/private-interface override. Host-local proxying is an operator decision.
        if !operations_bind.ip().is_loopback() {
            return Err(invalid(OPERATIONS_BIND));
        }
        let drain = seconds(get, DRAIN_SECONDS, 45)?;
        let stop = seconds(get, STOP_SECONDS, 60)?;
        if drain < 45 || Duration::from_secs(drain) <= crate::transport::REQUEST_TIMEOUT {
            return Err(invalid(DRAIN_SECONDS));
        }
        // Mirrors deploy/control RuntimeConfiguration: at least 15s outside the drain.
        // Cleanup uses at most 10s; leave the supervisor at least 5s before its kill.
        if stop < 60 || stop < drain + 15 {
            return Err(invalid(STOP_SECONDS));
        }
        Ok(Self {
            operations_bind,
            commit_sha: commit_sha.into(),
            drain: Duration::from_secs(drain),
        })
    }
}

struct StartupConfig {
    api: ApiConfig,
    lifecycle: LifecycleConfig,
    postgres: PostgresPoolConfig,
    opensearch: OpenSearch,
}

impl StartupConfig {
    fn from_env(api: ApiConfig) -> Result<Self, ApiStateError> {
        Self::from_getter(api, &mut env_input)
    }

    fn from_getter(
        api: ApiConfig,
        get: &mut impl FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, ApiStateError> {
        // All config, including protected PG files, precedes any network/provider discovery.
        let postgres = crate::postgres_config(get)?;
        let lifecycle = LifecycleConfig::from_getter(get)?;
        let stage = required(get, "STAGE")?;
        let real = matches!(stage.as_str(), "dev" | "prod");
        endpoint(&api.cognito_jwt.issuer, crate::COGNITO_ISSUER_ENV, real)?;
        endpoint(&api.cognito_jwt.jwks_url, crate::COGNITO_JWKS_URL_ENV, real)?;
        endpoint(&api.zoho.accounts_url, crate::ZOHO_ACCOUNTS_URL_ENV, real)?;
        endpoint(&api.zoho.campaigns_url, crate::ZOHO_CAMPAIGNS_URL_ENV, real)?;
        let search_endpoint = endpoint(
            &required(get, "OPENSEARCH_ENDPOINT_URL")?,
            "OPENSEARCH_ENDPOINT_URL",
            real,
        )?;
        let builder = TransportBuilder::new(SingleNodeConnectionPool::new(search_endpoint));
        let builder = if stage == "ephemeral" {
            builder
        } else {
            builder.auth(Credentials::Basic(
                required(get, "OPENSEARCH_USERNAME")?,
                required(get, "OPENSEARCH_PASSWORD")?,
            ))
        };
        let transport = builder.build().map_err(|_| ApiStateError::OpenSearch)?;
        Ok(Self {
            api,
            lifecycle,
            postgres,
            opensearch: OpenSearch::new(transport),
        })
    }
}

pub(crate) fn env_input(name: &'static str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => Some(String::new()),
    }
}

fn invalid(name: &'static str) -> ApiStateError {
    ApiStateError::RuntimeConfig { name }
}

fn required(
    get: &mut impl FnMut(&'static str) -> Option<String>,
    name: &'static str,
) -> Result<String, ApiStateError> {
    get(name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid(name))
}

fn seconds(
    get: &mut impl FnMut(&'static str) -> Option<String>,
    name: &'static str,
    default: u64,
) -> Result<u64, ApiStateError> {
    let Some(raw) = get(name) else {
        return Ok(default);
    };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid(name));
    }
    let value = raw.parse::<u64>().map_err(|_| invalid(name))?;
    if !(1..=86400).contains(&value) {
        return Err(invalid(name));
    }
    Ok(value)
}

fn endpoint(raw: &str, name: &'static str, real: bool) -> Result<url::Url, ApiStateError> {
    let url = url::Url::parse(raw).map_err(|_| invalid(name))?;
    if !matches!(url.scheme(), "http" | "https")
        || (real && url.scheme() != "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(name));
    }
    Ok(url)
}

async fn required_dependencies(config: &StartupConfig, pool: &PgPool) -> Result<(), ApiStateError> {
    // Shared helper owns exact embedded SQLx checksums/history/extensions. Never migrate here.
    verify_business_schema(pool).await?;
    check_search(&config.opensearch).await?;
    check_jwks(&config.api.cognito_jwt.jwks_url).await
}

async fn check_search(search: &OpenSearch) -> Result<(), ApiStateError> {
    let response = timeout(DEPENDENCY_TIMEOUT, search.ping().send())
        .await
        .map_err(|_| ApiStateError::Dependency { name: "OPENSEARCH" })?
        .map_err(|_| ApiStateError::Dependency { name: "OPENSEARCH" })?;
    if !response.status_code().is_success() {
        return Err(ApiStateError::Dependency { name: "OPENSEARCH" });
    }
    Ok(())
}

async fn check_jwks(url: &str) -> Result<(), ApiStateError> {
    let failure = || ApiStateError::Dependency {
        name: "COGNITO_JWKS",
    };
    let client = reqwest::Client::builder()
        .connect_timeout(crate::JWKS_CONNECT_TIMEOUT)
        .timeout(crate::JWKS_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| failure())?;
    let mut response = client.get(url).send().await.map_err(|_| failure())?;
    if !response.status().is_success() {
        return Err(failure());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
        if body.len() + chunk.len() > 65536 {
            return Err(failure());
        }
        body.extend_from_slice(&chunk);
    }
    let keys: crate::auth::JsonWebKeySet = serde_json::from_slice(&body).map_err(|_| failure())?;
    if keys.keys.is_empty() {
        return Err(failure());
    }
    Ok(())
}

async fn close_pool(pool: Option<&PgPool>) -> Result<(), ApiStateError> {
    if let Some(pool) = pool {
        timeout(POOL_CLOSE_TIMEOUT, pool.close())
            .await
            .map_err(|_| ApiStateError::PoolCloseDeadline)?;
    }
    Ok(())
}

/// Read-only preflight. Does not bind listeners, discover cloud credentials, construct
/// business adapters, invoke inference, synchronize scopes, or apply schema changes.
pub async fn check_config(api: ApiConfig) -> Result<(), ApiRunError> {
    let config = StartupConfig::from_env(api)?;
    let mut pool = None;
    let result = timeout(PREFLIGHT_TIMEOUT, async {
        pool = Some(config.postgres.connect().await?);
        let connected = pool.as_ref().ok_or(ApiStateError::StartupDeadline)?;
        required_dependencies(&config, connected).await
    })
    .await
    .map_err(|_| ApiStateError::StartupDeadline)
    .and_then(|result| result);
    let closed = close_pool(pool.as_ref()).await;
    result.and(closed).map_err(ApiRunError::State)
}

async fn compose(config: &StartupConfig, pool: &PgPool) -> Result<AppState, ApiStateError> {
    // No provider discovery or domain adapter construction belongs to preflight.
    let cloud = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .load()
        .await;
    // Google auth starts background token refresh on construction. Keep provider
    // initialization explicitly outside both preflight and business adapter composition.
    let google = crate::google_application_default_credentials()?;
    app_state_from_config(
        &config.api,
        pool.clone(),
        config.opensearch.clone(),
        cloud,
        google,
    )
}

pub(crate) async fn build_state(api: ApiConfig) -> Result<AppState, ApiStateError> {
    let config = StartupConfig::from_env(api)?;
    let mut pool = None;
    let result = timeout(STARTUP_TIMEOUT, async {
        pool = Some(config.postgres.connect().await?);
        let connected = pool.as_ref().ok_or(ApiStateError::StartupDeadline)?;
        required_dependencies(&config, connected).await?;
        compose(&config, connected).await
    })
    .await
    .map_err(|_| ApiStateError::StartupDeadline)
    .and_then(|result| result);
    if result.is_err() {
        close_pool(pool.as_ref()).await?;
    }
    result
}

/// Owns every HTTP/1 connection task, including its request future and response body.
/// Unlike axum::serve's detached tasks, abort + join here also drops stuck handlers.
struct HttpServer {
    listener: Option<TcpListener>,
    router: Router,
    stop: watch::Sender<bool>,
    connections: JoinSet<()>,
    accept_retry_at: Option<Instant>,
    accept_retry_attempt: u64,
}

impl HttpServer {
    fn new(listener: TcpListener, router: Router) -> Self {
        let (stop, _) = watch::channel(false);
        Self {
            listener: Some(listener),
            router,
            stop,
            connections: JoinSet::new(),
            accept_retry_at: None,
            accept_retry_attempt: 0,
        }
    }

    async fn step(&mut self) -> Result<(), ApiRunError> {
        let listener = self.listener.as_ref().ok_or(ApiRunError::ConnectionTask)?;
        tokio::select! {
            biased;
            result = self.connections.join_next(), if !self.connections.is_empty() => {
                if let Some(Err(_)) = result { return Err(ApiRunError::ConnectionTask); }
            }
            accepted = async {
                // step() is cancelled whenever another listener/probe wins the outer select.
                // Retain an absolute deadline so that cancellation cannot erase the backoff.
                if let Some(retry_at) = self.accept_retry_at {
                    tokio::time::sleep_until(retry_at).await;
                }
                listener.accept().await
            } => {
                let (stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        let delay = accept_retry_delay(&error);
                        self.accept_retry_at = Some(Instant::now() + delay);
                        self.accept_retry_attempt = self.accept_retry_attempt.saturating_add(1);
                        if !delay.is_zero() {
                            tracing::warn!(
                                event = "api.accept_retry",
                                error_kind = ?error.kind(),
                                os_error = error.raw_os_error(),
                                retry_attempt = self.accept_retry_attempt,
                                delay_ms = delay.as_millis() as u64,
                            );
                        }
                        return Ok(());
                    }
                };
                self.accept_retry_at = None;
                self.accept_retry_attempt = 0;
                let router = self.router.clone();
                let mut stop = self.stop.subscribe();
                self.connections.spawn(async move {
                    // Current axum configuration supports HTTP/1 only, no upgrades or HTTP/2.
                    // Hyper HTTP/1 polls the service future inline; no request executor detaches it.
                    let connection = http1::Builder::new().serve_connection(TokioIo::new(stream), TowerToHyperService::new(router));
                    tokio::pin!(connection);
                    tokio::select! {
                        biased;
                        _ = stop.changed() => connection.as_mut().graceful_shutdown(),
                        result = &mut connection => {
                            record_connection_result(result);
                            return;
                        }
                    }
                    record_connection_result(connection.await);
                });
            }
        }
        Ok(())
    }

    fn stop_accepting(&mut self) {
        self.listener.take();
        self.stop.send_replace(true);
    }

    async fn join(&mut self) -> Result<(), ApiRunError> {
        let mut failed = false;
        while let Some(result) = self.connections.join_next().await {
            if let Err(error) = result {
                failed |= !error.is_cancelled();
            }
        }
        if failed {
            Err(ApiRunError::ConnectionTask)
        } else {
            Ok(())
        }
    }

    async fn finish(
        &mut self,
        drain_deadline: Instant,
        cleanup_deadline: Instant,
    ) -> Result<(), ApiRunError> {
        self.stop_accepting();
        match timeout_at(drain_deadline, self.join()).await {
            Ok(result) => result,
            Err(_) => {
                self.connections.abort_all();
                timeout_at(cleanup_deadline, self.join())
                    .await
                    .map_err(|_| ApiRunError::CleanupDeadline)??;
                Err(ApiRunError::DrainDeadline)
            }
        }
    }
}

fn accept_retry_delay(error: &std::io::Error) -> Duration {
    // Match axum's listener policy: peer errors retry immediately; resource/other
    // errors (including EMFILE/ENFILE) retry after one second, never end READY.
    match error.kind() {
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::ConnectionReset => Duration::ZERO,
        _ => Duration::from_secs(1),
    }
}

fn record_connection_result(result: Result<(), hyper::Error>) {
    if result.is_err() {
        // Peer disconnects and malformed HTTP are normal transport failures. Never log URI/body/error.
        tracing::debug!(event = "api.connection_closed", outcome = "transport_error");
    }
}

fn api_router(state: AppState, lifecycle: Arc<Lifecycle>) -> Router {
    admission(crate::business_routes(state), lifecycle)
}

fn admission(router: Router, lifecycle: Arc<Lifecycle>) -> Router {
    crate::transport::with_transport_middleware(router.layer(axum::middleware::from_fn(
        move |request: Request, next: Next| {
            let lifecycle = lifecycle.clone();
            async move {
                // Work admitted before DRAINING keeps its ordinary request timeout and transaction semantics.
                if lifecycle.state() != ApiLifecycleState::Ready {
                    return (
                        http::StatusCode::SERVICE_UNAVAILABLE,
                        [(http::header::CACHE_CONTROL, "no-store")],
                    )
                        .into_response();
                }
                next.run(request).await
            }
        },
    )))
}

async fn while_serving_probes<F: Future>(
    future: F,
    operations: &mut HttpServer,
) -> (F::Output, Result<(), ApiRunError>) {
    tokio::pin!(future);
    let mut operational_result = Ok(());
    loop {
        tokio::select! {
            biased;
            result = &mut future => return (result, operational_result),
            result = operations.step(), if operational_result.is_ok() => {
                if result.is_err() {
                    operations.stop_accepting();
                    operational_result = result;
                }
            }
        }
    }
}

async fn finish_runtime(
    api: Option<&mut HttpServer>,
    operations: &mut HttpServer,
    lifecycle: &Lifecycle,
    drain: Duration,
    pool: Option<&PgPool>,
) -> Result<(), ApiRunError> {
    lifecycle.begin_drain();
    tracing::info!(state = "DRAINING", "API lifecycle");
    let drain_deadline = Instant::now() + drain;
    let final_deadline = drain_deadline + CLEANUP_RESERVE;
    let api_result = if let Some(api) = api {
        api.stop_accepting();
        let (result, probes) = while_serving_probes(
            api.finish(drain_deadline, drain_deadline + TASK_CLOSE_TIMEOUT),
            operations,
        )
        .await;
        result.and(probes)
    } else {
        Ok(())
    };
    let (pool_result, probes) = while_serving_probes(close_pool(pool), operations).await;
    lifecycle.mark_stopped();
    operations.stop_accepting();
    let operations_result = operations
        .finish(
            (Instant::now() + TASK_CLOSE_TIMEOUT).min(final_deadline),
            final_deadline,
        )
        .await;
    tracing::info!(state = "STOPPED", "API lifecycle");
    api_result
        .and(pool_result.map_err(ApiRunError::State))
        .and(probes)
        .and(operations_result)
}

pub async fn run_until_shutdown<S>(api: ApiConfig, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let config = StartupConfig::from_env(api)?;
    crate::log_product_listing_search_cache_config(&config.api);
    let lifecycle = Arc::new(Lifecycle::new());
    let operations_listener = TcpListener::bind(config.lifecycle.operations_bind)
        .await
        .map_err(ApiRunError::Bind)?;
    let mut operations = HttpServer::new(
        operations_listener,
        crate::operations::router(OperationsState {
            lifecycle: lifecycle.clone(),
            commit_sha: config.lifecycle.commit_sha.clone(),
        }),
    );
    tracing::info!(state = "STARTING", "API lifecycle");
    tokio::pin!(shutdown);
    let mut pool = None;
    // Keeping pool ownership outside this cancellable future makes partial startup closable.
    let startup_result = {
        let startup = timeout(STARTUP_TIMEOUT, async {
            pool = Some(
                config
                    .postgres
                    .connect()
                    .await
                    .map_err(ApiStateError::from)?,
            );
            let connected = pool.as_ref().ok_or(ApiStateError::StartupDeadline)?;
            required_dependencies(&config, connected).await?;
            let state = compose(&config, connected).await?;
            let listener = TcpListener::bind(config.api.bind_addr())
                .await
                .map_err(ApiRunError::Bind)?;
            Ok::<_, ApiRunError>(HttpServer::new(
                listener,
                api_router(state, lifecycle.clone()),
            ))
        });
        tokio::pin!(startup);
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => break Ok(None),
                result = &mut startup => break result.map_err(|_| ApiRunError::State(ApiStateError::StartupDeadline)).and_then(|result| result).map(Some),
                result = operations.step() => if let Err(error) = result { break Err(error); },
            }
        }
    };
    let mut api = match startup_result {
        Ok(Some(api)) => api,
        result => {
            let cleanup = finish_runtime(
                None,
                &mut operations,
                &lifecycle,
                config.lifecycle.drain,
                pool.as_ref(),
            )
            .await;
            return result.map(|_| ()).and(cleanup);
        }
    };
    lifecycle.mark_ready();
    tracing::info!(state = "READY", "API lifecycle");
    let connected = pool.as_ref().ok_or(ApiStateError::StartupDeadline)?;
    let readiness = RuntimeReadiness {
        postgres: connected.clone(),
        opensearch: config.opensearch.clone(),
    };
    let outcome = run_ready(
        &mut api,
        &mut operations,
        &lifecycle,
        &readiness,
        shutdown.as_mut(),
    )
    .await;
    let cleanup = finish_runtime(
        Some(&mut api),
        &mut operations,
        &lifecycle,
        config.lifecycle.drain,
        pool.as_ref(),
    )
    .await;
    outcome.and(cleanup)
}

async fn run_ready<S: Future<Output = ()>>(
    api: &mut HttpServer,
    operations: &mut HttpServer,
    lifecycle: &Lifecycle,
    readiness: &dyn ReadinessCheck,
    mut shutdown: Pin<&mut S>,
) -> Result<(), ApiRunError> {
    let refresh = async {
        loop {
            tokio::time::sleep(DEPENDENCY_TIMEOUT).await;
            let ready = matches!(
                timeout(DEPENDENCY_TIMEOUT, readiness.check()).await,
                Ok(Ok(()))
            );
            lifecycle.dependencies_ready.store(ready, Ordering::SeqCst);
        }
    };
    tokio::pin!(refresh);
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => return Ok(()),
            _ = &mut refresh => return Err(ApiRunError::ConnectionTask),
            result = operations.step() => result?,
            result = api.step() => result?,
        }
    }
}

/// Transport-neutral HTTP serving boundary; preserves the supplied router's middleware.
/// Production composes lifecycle admission, common transport, private probes and pool cleanup.
pub async fn serve<S>(listener: TcpListener, router: Router, shutdown: S) -> Result<(), ApiRunError>
where
    S: Future<Output = ()> + Send + 'static,
{
    let mut server = HttpServer::new(listener, router);
    tokio::pin!(shutdown);
    let outcome = loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break Ok(()),
            result = server.step() => if result.is_err() { break result; },
        }
    };
    let deadline = Instant::now() + Duration::from_secs(45);
    let cleanup = server.finish(deadline, deadline + TASK_CLOSE_TIMEOUT).await;
    outcome.and(cleanup)
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

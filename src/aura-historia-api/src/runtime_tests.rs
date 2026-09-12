use super::*;
use crate::app;
use axum::{body::Body, routing::get};
use http::{Request as HttpRequest, StatusCode};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::{Command, Stdio},
};
use tower::ServiceExt;

#[path = "signals.rs"]
mod signals;
#[path = "runtime_test_support.rs"]
mod support;
use support::{OwnedChild, OwnedDirectory, TestResult, inputs, wait_for_file};
#[path = "local_postgres_fixture.rs"]
mod local_postgres_fixture;

#[tokio::test]
#[ignore = "opt-in: owned local PostgreSQL container, cached image only"]
async fn should_bound_pool_close_when_a_connection_is_still_checked_out() -> TestResult {
    let postgres = local_postgres_fixture::LocalPostgres::start().await?;
    let connection = postgres.admin.acquire().await?;
    let started = Instant::now();
    assert!(matches!(
        close_pool(Some(&postgres.admin)).await,
        Err(ApiStateError::PoolCloseDeadline)
    ));
    assert!(started.elapsed() < Duration::from_secs(7));
    assert!(postgres.admin.is_closed());
    assert!(postgres.port > 0);
    timeout(Duration::from_secs(2), connection.close()).await??;
    postgres.close().await?;
    Ok(())
}

struct StalledBody {
    first: bool,
    dropped: Arc<AtomicBool>,
}

impl hyper::body::Body for StalledBody {
    type Data = axum::body::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        if self.first {
            self.first = false;
            std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(
                axum::body::Bytes::from_static(b"first"),
            ))))
        } else {
            std::task::Poll::Pending
        }
    }
}

impl Drop for StalledBody {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn should_abort_and_join_stalled_response_bodies_at_the_drain_deadline() -> TestResult {
    let dropped = Arc::new(AtomicBool::new(false));
    let marker = dropped.clone();
    let router = Router::new().route(
        "/",
        get(move || async move {
            Body::new(StalledBody {
                first: true,
                dropped: marker,
            })
        }),
    );
    let lifecycle = Arc::new(Lifecycle::new());
    lifecycle.mark_ready();
    let router = admission(router, lifecycle);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (stop, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut server = HttpServer::new(listener, router);
        tokio::pin!(receiver);
        loop {
            tokio::select! {
                _ = &mut receiver => break,
                result = server.step() => result?,
            }
        }
        let deadline = Instant::now() + Duration::from_millis(50);
        let result = server
            .finish(deadline, deadline + Duration::from_secs(1))
            .await;
        assert!(server.connections.is_empty());
        result
    });
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()?
        .get(format!("http://{address}/"))
        .send()
        .await?;
    stop.send(()).map_err(|_| "server stopped early")?;
    assert!(matches!(
        timeout(Duration::from_secs(2), server).await??,
        Err(ApiRunError::DrainDeadline)
    ));
    assert!(dropped.load(Ordering::SeqCst));
    assert!(response.bytes().await.is_err());
    Ok(())
}

#[rstest::rstest]
#[case(std::io::ErrorKind::ConnectionRefused, Duration::ZERO)]
#[case(std::io::ErrorKind::ConnectionAborted, Duration::ZERO)]
#[case(std::io::ErrorKind::ConnectionReset, Duration::ZERO)]
#[case(std::io::ErrorKind::Interrupted, Duration::from_secs(1))]
#[case(std::io::ErrorKind::WouldBlock, Duration::from_secs(1))]
#[case(std::io::ErrorKind::PermissionDenied, Duration::from_secs(1))]
#[case(std::io::ErrorKind::InvalidInput, Duration::from_secs(1))]
#[case(std::io::ErrorKind::Other, Duration::from_secs(1))]
fn should_classify_accept_errors_like_axum(
    #[case] kind: std::io::ErrorKind,
    #[case] delay: Duration,
) {
    assert_eq!(accept_retry_delay(&std::io::Error::from(kind)), delay);
}

#[cfg(target_os = "linux")]
#[rstest::rstest]
#[case::emfile(24)]
#[case::enfile(23)]
#[case::enomem(12)]
#[case::enobufs(105)]
fn should_back_off_when_accept_exhausts_os_resources(#[case] errno: i32) {
    assert_eq!(
        accept_retry_delay(&std::io::Error::from_raw_os_error(errno)),
        Duration::from_secs(1)
    );
}

#[tokio::test]
async fn should_retain_accept_backoff_across_cancellation_and_connection_joins() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let client = tokio::net::TcpStream::connect(listener.local_addr()?).await?;
    let mut server = HttpServer::new(listener, Router::new());
    let retry_at = Instant::now() + Duration::from_millis(200);
    server.accept_retry_at = Some(retry_at);
    server.accept_retry_attempt = 3;
    for _ in 0..3 {
        assert!(
            timeout(Duration::from_millis(10), server.step())
                .await
                .is_err()
        );
        assert_eq!(server.accept_retry_at, Some(retry_at));
        assert!(server.connections.is_empty());
    }
    server.connections.spawn(async {});
    timeout(Duration::from_millis(50), server.step()).await??;
    assert!(server.connections.is_empty());
    assert_eq!(server.accept_retry_at, Some(retry_at));
    timeout(Duration::from_secs(1), server.step()).await??;
    assert!(Instant::now() >= retry_at);
    assert_eq!(server.accept_retry_at, None);
    assert_eq!(server.accept_retry_attempt, 0);
    assert_eq!(server.connections.len(), 1);
    drop(client);
    let deadline = Instant::now() + Duration::from_secs(1);
    server.finish(deadline, deadline).await?;
    Ok(())
}

#[tokio::test]
async fn should_cancel_accept_backoff_without_delaying_shutdown() -> TestResult {
    let mut api = HttpServer::new(TcpListener::bind("127.0.0.1:0").await?, Router::new());
    let mut operations = HttpServer::new(TcpListener::bind("127.0.0.1:0").await?, Router::new());
    let retry_at = Instant::now() + Duration::from_secs(60);
    api.accept_retry_at = Some(retry_at);
    operations.accept_retry_at = Some(retry_at);
    let lifecycle = Lifecycle::new();
    lifecycle.mark_ready();
    let shutdown = tokio::time::sleep(Duration::from_millis(10));
    tokio::pin!(shutdown);
    timeout(
        Duration::from_millis(200),
        run_ready(
            &mut api,
            &mut operations,
            &lifecycle,
            &Healthy,
            shutdown.as_mut(),
        ),
    )
    .await??;
    assert!(lifecycle.is_ready());
    timeout(
        Duration::from_millis(200),
        finish_runtime(
            Some(&mut api),
            &mut operations,
            &lifecycle,
            Duration::from_secs(45),
            None,
        ),
    )
    .await??;
    assert_eq!(lifecycle.state(), ApiLifecycleState::Stopped);
    assert!(api.connections.is_empty());
    assert!(operations.connections.is_empty());
    Ok(())
}

#[derive(Default)]
struct AdmissionWitness {
    calls: std::sync::atomic::AtomicUsize,
    authentications: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::auth::TokenAuthenticator for AdmissionWitness {
    async fn authenticate(
        &self,
        _: &str,
        _: &crate::auth::RequestMetadata,
    ) -> Result<crate::auth::TransportPrincipal, crate::auth::AuthError> {
        self.authentications.fetch_add(1, Ordering::SeqCst);
        Err(crate::auth::AuthError::InvalidCredentials)
    }
}

#[async_trait::async_trait]
impl user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionUseCase
    for AdmissionWitness
{
    async fn execute(
        &self,
        _: &application::operation_context::OperationContext,
        _: user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionCommand,
    ) -> Result<(), user_service::use_cases::commands::upsert_newsletter_subscription::UpsertNewsletterSubscriptionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[rstest::rstest]
#[case::missing(None)]
#[case::valid(Some("trace_123-abc".into()))]
#[case::invalid(Some("contains space".into()))]
#[case::oversized(Some("a".repeat(129)))]
#[tokio::test]
async fn should_preserve_transport_headers_on_drain_rejection_through_business_router(
    #[case] correlation_id: Option<String>,
) -> TestResult {
    use crate::transport::{CORRELATION_ID_HEADER, REQUEST_ID_HEADER};
    use http::{Method, header};

    let lifecycle = Arc::new(Lifecycle::new());
    lifecycle.mark_ready();
    let witness = Arc::new(AdmissionWitness::default());
    let state = AppState::new().with_newsletter(crate::state::NewsletterState::new(
        witness.clone(),
        witness.clone(),
    ));
    let router = api_router(state, lifecycle.clone());
    let request = || {
        let mut request = HttpRequest::builder()
            .method(Method::PUT)
            .uri("/api/v1/newsletter-subscriptions")
            .header(header::ORIGIN, "https://example.test")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(value) = correlation_id.as_deref() {
            request = request.header(CORRELATION_ID_HEADER, value);
        }
        request
    };
    let response = router
        .clone()
        .oneshot(request().body(Body::from(r#"{"email":"test@example.test"}"#))?)
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    // Dependency sampling failure must not become a business admission policy.
    lifecycle.dependencies_ready.store(false, Ordering::SeqCst);
    let response = router
        .clone()
        .oneshot(
            request()
                .header(header::AUTHORIZATION, "Bearer rejected")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(witness.calls.load(Ordering::SeqCst), 1);
    assert_eq!(witness.authentications.load(Ordering::SeqCst), 1);

    lifecycle.begin_drain();
    for stopped in [false, true] {
        if stopped {
            lifecycle.mark_stopped();
        }
        for path in ["/api/v1/newsletter-subscriptions", "/not-found"] {
            let response = router
                .clone()
                .oneshot(
                    request()
                        .uri(path)
                        .header(header::AUTHORIZATION, "Bearer rejected")
                        .body(Body::empty())?,
                )
                .await?;
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let headers = response.headers();
            assert_eq!(headers[header::CACHE_CONTROL], "no-store");
            assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
            let request_id = headers[&REQUEST_ID_HEADER].to_str()?;
            uuid::Uuid::parse_str(request_id)?;
            assert_eq!(
                headers[&CORRELATION_ID_HEADER],
                if correlation_id.as_deref() == Some("trace_123-abc") {
                    "trace_123-abc"
                } else {
                    request_id
                }
            );
            let exposed = headers[header::ACCESS_CONTROL_EXPOSE_HEADERS].to_str()?;
            for name in [REQUEST_ID_HEADER, CORRELATION_ID_HEADER] {
                assert!(
                    exposed
                        .split(',')
                        .any(|value| value.trim().eq_ignore_ascii_case(name.as_str()))
                );
            }
            assert!(
                axum::body::to_bytes(response.into_body(), 1024)
                    .await?
                    .is_empty()
            );
        }
    }
    assert_eq!(witness.calls.load(Ordering::SeqCst), 1);
    assert_eq!(witness.authentications.load(Ordering::SeqCst), 1);
    let response = router
        .oneshot(
            HttpRequest::builder()
                .method(Method::OPTIONS)
                .uri("/api/v1/newsletter-subscriptions")
                .header(header::ORIGIN, "https://example.test")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
    assert!(response.headers().contains_key(REQUEST_ID_HEADER));
    assert!(response.headers().contains_key(CORRELATION_ID_HEADER));
    Ok(())
}

#[tokio::test]
async fn should_preserve_supplied_transport_metadata_through_low_level_serve() -> TestResult {
    use crate::transport::{CORRELATION_ID_HEADER, REQUEST_ID_HEADER};

    let router =
        crate::transport::with_transport_middleware(Router::new().route(
            "/",
            get(|headers: http::HeaderMap| async move {
                headers[&REQUEST_ID_HEADER].as_bytes().to_vec()
            }),
        ));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (stop, receiver) = tokio::sync::oneshot::channel();
    let mut tasks = JoinSet::new();
    tasks.spawn(serve(listener, router, async {
        let _closed = receiver.await;
    }));
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()?
        .get(format!("http://{address}/"))
        .header(CORRELATION_ID_HEADER, "wire-correlation")
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[&CORRELATION_ID_HEADER],
        "wire-correlation"
    );
    let request_id = response.headers()[&REQUEST_ID_HEADER].to_str()?.to_owned();
    uuid::Uuid::parse_str(&request_id)?;
    assert_eq!(response.text().await?, request_id);
    stop.send(()).map_err(|_| "server stopped early")?;
    timeout(Duration::from_secs(2), tasks.join_next())
        .await?
        .ok_or("missing server task")???;
    assert!(tasks.is_empty());
    Ok(())
}

#[tokio::test]
async fn should_keep_admitted_request_timeout_and_headers_when_draining() -> TestResult {
    let lifecycle = Arc::new(Lifecycle::new());
    lifecycle.mark_ready();
    let entered = Arc::new(tokio::sync::Notify::new());
    let marker = entered.clone();
    let router = admission(
        Router::new().route(
            "/",
            get(move || async move {
                marker.notify_one();
                std::future::pending::<StatusCode>().await
            }),
        ),
        lifecycle.clone(),
    );
    let request = router.oneshot(
        HttpRequest::builder()
            .uri("/")
            .header(http::header::ORIGIN, "https://example.test")
            .body(Body::empty())?,
    );
    tokio::pin!(request);
    tokio::select! {
        _ = entered.notified() => lifecycle.begin_drain(),
        _ = &mut request => return Err("request ended before admission witness".into()),
        _ = tokio::time::sleep(Duration::from_secs(2)) => return Err("request not admitted".into()),
    }
    let response = timeout(
        crate::transport::REQUEST_TIMEOUT + Duration::from_secs(2),
        request,
    )
    .await??;
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(
        response.headers()[http::header::ACCESS_CONTROL_ALLOW_ORIGIN],
        "*"
    );
    assert!(
        response
            .headers()
            .contains_key(crate::transport::REQUEST_ID_HEADER)
    );
    assert!(
        response
            .headers()
            .contains_key(crate::transport::CORRELATION_ID_HEADER)
    );
    Ok(())
}

fn parse(values: &BTreeMap<&'static str, String>) -> Result<StartupConfig, ApiStateError> {
    let api = ApiConfig::from_getter(|key| values.get(key).cloned())?;
    StartupConfig::from_getter(api, &mut |key| values.get(key).cloned())
}

#[test]
fn should_use_private_probes_and_derive_validated_shutdown_budgets() -> TestResult {
    let mut values = inputs();
    values.remove(OPERATIONS_BIND);
    let config = parse(&values)?;
    assert_eq!(
        config.lifecycle.operations_bind,
        "127.0.0.1:9080".parse::<SocketAddr>()?
    );
    assert_eq!(config.lifecycle.drain, Duration::from_secs(45));
    assert!(crate::transport::REQUEST_TIMEOUT < config.lifecycle.drain);
    assert!(CLEANUP_RESERVE < Duration::from_secs(15));
    Ok(())
}

#[rstest::rstest]
#[case("0.0.0.0:9080")]
#[case("[::]:9080")]
#[case("192.168.1.2:9080")]
#[case("8.8.8.8:9080")]
#[case("localhost:9080")]
#[case("secret_canary")]
fn should_reject_non_loopback_or_invalid_operational_listener(#[case] addr: &str) -> TestResult {
    let mut values = inputs();
    values.insert(OPERATIONS_BIND, addr.into());
    let error = parse(&values).err().ok_or("expected config rejection")?;
    assert!(matches!(
        error,
        ApiStateError::RuntimeConfig {
            name: OPERATIONS_BIND
        }
    ));
    assert!(!format!("{error:?} {error}").contains(addr));
    Ok(())
}

#[rstest::rstest]
#[case(DRAIN_SECONDS, "30")]
#[case(DRAIN_SECONDS, "44")]
#[case(DRAIN_SECONDS, "0")]
#[case(DRAIN_SECONDS, "+45")]
#[case(DRAIN_SECONDS, "45 ")]
#[case(DRAIN_SECONDS, "")]
#[case(STOP_SECONDS, "59")]
#[case(STOP_SECONDS, "18446744073709551615")]
fn should_reject_unsafe_or_malformed_shutdown_budget(
    #[case] name: &'static str,
    #[case] value: &str,
) {
    let mut values = inputs();
    values.insert(name, value.into());
    assert!(parse(&values).is_err());
    values.insert(DRAIN_SECONDS, "60".into());
    values.insert(STOP_SECONDS, "74".into());
    assert!(parse(&values).is_err());
    values.insert(STOP_SECONDS, "75".into());
    assert!(parse(&values).is_ok());
}

#[rstest::rstest]
#[case("dev")]
#[case("prod")]
#[case("local")]
#[case("test")]
#[case("ephemeral")]
fn should_require_full_lowercase_sha_or_explicit_non_real_fallback(
    #[case] stage: &str,
) -> TestResult {
    let mut values = inputs();
    values.insert("STAGE", stage.into());
    for sha in [
        "main",
        "v1",
        "D5BD9CA854E713B0C587528F02037211B2020FD4",
        "d5bd9ca",
        "",
        "secret_canary",
    ] {
        values.insert("COMMIT_SHA", sha.into());
        assert!(LifecycleConfig::from_getter(&mut |key| values.get(key).cloned()).is_err());
    }
    values.remove("COMMIT_SHA");
    assert!(LifecycleConfig::from_getter(&mut |key| values.get(key).cloned()).is_err());
    values.insert(
        "COMMIT_SHA",
        "d5bd9ca854e713b0c587528f02037211b2020fd4".into(),
    );
    LifecycleConfig::from_getter(&mut |key| values.get(key).cloned())?;
    values.insert("COMMIT_SHA", "unversioned".into());
    assert_eq!(
        !matches!(stage, "dev" | "prod"),
        LifecycleConfig::from_getter(&mut |key| values.get(key).cloned()).is_ok()
    );
    Ok(())
}

#[test]
fn should_validate_postgres_once_before_later_startup_inputs() -> TestResult {
    let mut values = inputs();
    let api = ApiConfig::from_getter(|key| values.get(key).cloned())?;
    values.insert("POSTGRES_SSL_MODE", "prefer".into());
    let mut reads = BTreeMap::new();
    let error = StartupConfig::from_getter(api, &mut |key| {
        *reads.entry(key).or_insert(0) += 1;
        values.get(key).cloned()
    })
    .err()
    .ok_or("expected PG failure")?;
    assert!(matches!(error, ApiStateError::PostgresConfig(_)));
    assert!(!reads.contains_key("COMMIT_SHA"));
    assert!(!reads.contains_key("OPENSEARCH_ENDPOINT_URL"));
    assert!(reads.values().all(|count| *count == 1));
    Ok(())
}

#[test]
fn should_keep_lifecycle_monotonic_and_fail_readiness_closed() {
    let lifecycle = Lifecycle::new();
    assert_eq!(lifecycle.state(), ApiLifecycleState::Starting);
    assert!(!lifecycle.is_ready());
    lifecycle.mark_ready();
    assert!(lifecycle.is_ready());
    lifecycle.dependencies_ready.store(false, Ordering::SeqCst);
    assert!(!lifecycle.is_ready());
    lifecycle.begin_drain();
    lifecycle.mark_ready();
    assert_eq!(lifecycle.state(), ApiLifecycleState::Draining);
    assert!(!lifecycle.is_ready());
    lifecycle.mark_stopped();
    lifecycle.begin_drain();
    lifecycle.mark_ready();
    assert_eq!(lifecycle.state(), ApiLifecycleState::Stopped);
    assert!(!lifecycle.is_ready());
}

#[tokio::test]
async fn should_expose_only_safe_noncacheable_private_probes() -> TestResult {
    let lifecycle = Arc::new(Lifecycle::new());
    let operations = crate::operations::router(OperationsState {
        lifecycle: lifecycle.clone(),
        commit_sha: "unversioned".into(),
    });
    for (expected, ready) in [
        (ApiLifecycleState::Starting, false),
        (ApiLifecycleState::Ready, true),
        (ApiLifecycleState::Draining, false),
        (ApiLifecycleState::Stopped, false),
    ] {
        match expected {
            ApiLifecycleState::Starting => {}
            ApiLifecycleState::Ready => lifecycle.mark_ready(),
            ApiLifecycleState::Draining => lifecycle.begin_drain(),
            ApiLifecycleState::Stopped => lifecycle.mark_stopped(),
        }
        for path in ["/health", "/ready", "/version", "/not-found"] {
            let response = operations
                .clone()
                .oneshot(HttpRequest::builder().uri(path).body(Body::empty())?)
                .await?;
            assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-store");
            if path == "/ready" {
                assert_eq!(response.status() == StatusCode::NO_CONTENT, ready);
            }
            if path == "/health" {
                assert_eq!(
                    response.status().is_success(),
                    expected != ApiLifecycleState::Stopped
                );
            }
            if path == "/version" {
                let body = axum::body::to_bytes(response.into_body(), 1024).await?;
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body)?,
                    serde_json::json!({"commit_sha":"unversioned", "state":expected.as_str()})
                );
            }
            let public = app(AppState::new())
                .oneshot(HttpRequest::builder().uri(path).body(Body::empty())?)
                .await?;
            assert_eq!(public.status(), StatusCode::NOT_FOUND);
        }
        let response = operations
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/version")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-store");
    }
    Ok(())
}

struct Healthy;
#[async_trait::async_trait]
impl ReadinessCheck for Healthy {
    async fn check(&self) -> Result<(), ()> {
        Ok(())
    }
}

struct RequestDropped(PathBuf);
impl Drop for RequestDropped {
    fn drop(&mut self) {
        if let Err(error) = std::fs::write(&self.0, "dropped") {
            eprintln!("test marker failed: {}", error.kind());
        }
    }
}

// Private process helper exercises the production signal registration, connection owner,
// admission, probe router and drain code. It does NOT claim full dependency composition.
#[test]
#[ignore = "owned subprocess helper; invoked by parent process tests"]
fn process_helper() -> TestResult {
    let directory = PathBuf::from(std::env::var("API_LIFECYCLE_TEST_DIR")?);
    let mode = std::env::var("API_LIFECYCLE_TEST_MODE")?;
    if mode == "fd-exhaustion" {
        platform_observability::init(platform_observability::LoggingConfig::default());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        let signals = signals::ShutdownSignals::install()?;
        let lifecycle = Arc::new(Lifecycle::new());
        let request_directory = directory.clone();
        let router = Router::new()
            .route("/fast", get(|| async { StatusCode::NO_CONTENT }))
            .route(
                "/slow",
                get(move || {
                    let directory = request_directory.clone();
                    async move {
                        let _drop = RequestDropped(directory.join("request-dropped"));
                        if std::fs::write(directory.join("accepted"), "yes").is_err() {
                            return StatusCode::INTERNAL_SERVER_ERROR;
                        }
                        while !directory.join("release").exists() {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        if std::fs::write(directory.join("completed"), "yes").is_err() {
                            return StatusCode::INTERNAL_SERVER_ERROR;
                        }
                        StatusCode::NO_CONTENT
                    }
                }),
            );
        let api_listener = TcpListener::bind("127.0.0.1:0").await?;
        let api_address = api_listener.local_addr()?;
        let operations_listener = TcpListener::bind("127.0.0.1:0").await?;
        let operations_address = operations_listener.local_addr()?;
        let mut api = HttpServer::new(api_listener, admission(router, lifecycle.clone()));
        let mut operations = HttpServer::new(
            operations_listener,
            crate::operations::router(OperationsState {
                lifecycle: lifecycle.clone(),
                commit_sha: "unversioned".into(),
            }),
        );
        lifecycle.mark_ready();
        std::fs::write(
            directory.join("addresses"),
            format!("{api_address}\n{operations_address}"),
        )?;
        let shutdown = signals.wait();
        tokio::pin!(shutdown);
        let result = run_ready(
            &mut api,
            &mut operations,
            &lifecycle,
            &Healthy,
            shutdown.as_mut(),
        )
        .await;
        let drain = if mode == "deadline" {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(2)
        };
        let finished =
            finish_runtime(Some(&mut api), &mut operations, &lifecycle, drain, None).await;
        assert!(api.connections.is_empty());
        assert!(operations.connections.is_empty());
        assert_eq!(lifecycle.state(), ApiLifecycleState::Stopped);
        std::fs::write(directory.join("stopped"), "joined")?;
        result.and(finished).map_err(Into::into)
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

#[rstest::rstest]
#[case::interrupt_idle("-INT", "idle")]
#[case::terminate_idle("-TERM", "idle")]
#[case::interrupt_drain("-INT", "drain")]
#[case::terminate_drain("-TERM", "drain")]
#[case::interrupt_deadline("-INT", "deadline")]
#[case::terminate_deadline("-TERM", "deadline")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn should_bound_os_signal_shutdown_and_preserve_only_admitted_requests(
    #[case] signal: &str,
    #[case] mode: &str,
) -> TestResult {
    let directory = OwnedDirectory::new()?;
    let mut child = OwnedChild(
        Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "runtime::tests::process_helper",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("API_LIFECYCLE_TEST_DIR", &directory.0)
            .env("API_LIFECYCLE_TEST_MODE", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let addresses = wait_for_file(&directory.path("addresses")).await?;
    let mut addresses = addresses.lines();
    let api = format!("http://{}", addresses.next().ok_or("missing API address")?);
    let operations = format!(
        "http://{}",
        addresses.next().ok_or("missing operations address")?
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(4))
        .build()?;
    assert_eq!(
        client
            .get(format!("{operations}/ready"))
            .send()
            .await?
            .status(),
        StatusCode::NO_CONTENT
    );
    let accepted = if mode != "idle" {
        let client = client.clone();
        let url = format!("{api}/slow");
        let request = tokio::spawn(async move { client.get(url).send().await });
        wait_for_file(&directory.path("accepted")).await?;
        Some(request)
    } else {
        None
    };
    // A pre-existing idle connection cannot admit a fresh request during draining either.
    let mut idle = tokio::net::TcpStream::connect(api.trim_start_matches("http://")).await?;
    let started = Instant::now();
    child.signal(signal)?;
    if mode == "drain" {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let response = client.get(format!("{operations}/ready")).send().await?;
                assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-store");
                if response.status() == StatusCode::SERVICE_UNAVAILABLE {
                    return Ok::<_, reqwest::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await??;
        let version: serde_json::Value = client
            .get(format!("{operations}/version"))
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(version["state"], "DRAINING");
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if idle
            .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .is_ok()
        {
            let mut bytes = [0; 512];
            match tokio::time::timeout(Duration::from_secs(1), idle.read(&mut bytes)).await? {
                Ok(count) => {
                    assert!(!String::from_utf8_lossy(&bytes[..count]).contains("204 No Content"))
                }
                Err(error) => assert!(matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
                )),
            }
        }
        for _ in 0..8 {
            if let Ok(response) = client.get(format!("{api}/slow")).send().await {
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            }
        }
        std::fs::write(directory.path("release"), "yes")?;
    }
    if let Some(accepted) = accepted {
        let response = accepted.await?;
        if mode == "drain" {
            assert_eq!(response?.status(), StatusCode::NO_CONTENT);
        } else {
            assert!(response.is_err());
        }
        wait_for_file(&directory.path("request-dropped")).await?;
    }
    let status = child.wait(Duration::from_secs(5))?;
    assert_eq!(status.success(), mode != "deadline");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        std::fs::read_to_string(directory.path("stopped"))?,
        "joined"
    );
    assert_eq!(directory.path("completed").exists(), mode == "drain");
    Ok(())
}

#[cfg(target_os = "linux")]
fn fd_retry_count(directory: &OwnedDirectory) -> TestResult<usize> {
    Ok(std::fs::read_to_string(directory.path("output"))?
        .lines()
        .filter(|line| {
            line.contains(r#""event":"api.accept_retry""#) && line.contains(r#""os_error":24"#)
        })
        .count())
}

#[cfg(target_os = "linux")]
#[rstest::rstest]
#[case::api_recovery("-INT", false, true)]
#[case::operations_recovery("-TERM", true, true)]
#[case::interrupt_while_api_exhausted("-INT", false, false)]
#[case::terminate_while_operations_exhausted("-TERM", true, false)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn should_retry_real_fd_exhaustion_and_recover_or_stop_on_signal(
    #[case] signal: &str,
    #[case] exhaust_operations: bool,
    #[case] recover: bool,
) -> TestResult {
    let parent_limits = std::fs::read_to_string("/proc/self/limits")?;
    let directory = OwnedDirectory::new()?;
    let output = std::fs::File::create(directory.path("output"))?;
    // prlimit execs only this owned child: no unsafe pre_exec or parent/global limit changes.
    let mut child = OwnedChild(
        Command::new("prlimit")
            .arg("--nofile=64:64")
            .arg("--")
            .arg(std::env::current_exe()?)
            .args([
                "--exact",
                "runtime::tests::process_helper",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            .env("API_LIFECYCLE_TEST_DIR", &directory.0)
            .env("API_LIFECYCLE_TEST_MODE", "fd-exhaustion")
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output)
            .spawn()?,
    );
    let addresses = wait_for_file(&directory.path("addresses")).await?;
    let mut addresses = addresses.lines();
    let api = addresses.next().ok_or("missing API address")?;
    let operations = addresses.next().ok_or("missing operations address")?;
    let limits = std::fs::read_to_string(format!("/proc/{}/limits", child.0.id()))?;
    let nofile: Vec<_> = limits
        .lines()
        .find(|line| line.starts_with("Max open files"))
        .ok_or("missing child FD limit")?
        .split_whitespace()
        .collect();
    assert_eq!(&nofile[3..5], &["64", "64"]);
    let client = reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .timeout(Duration::from_secs(2))
        .build()?;
    // Reserve already-accepted connections on both listeners before exhausting the child.
    let response = client.get(format!("http://{api}/fast")).send().await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    response.bytes().await?;
    let version: serde_json::Value = client
        .get(format!("http://{operations}/version"))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(version["state"], "READY");

    let address = if exhaust_operations { operations } else { api };
    let mut connections = Vec::new();
    timeout(Duration::from_secs(5), async {
        for _ in 0..80 {
            assert!(
                child.0.try_wait()?.is_none(),
                "child exited during FD exhaustion"
            );
            if fd_retry_count(&directory)? > 0 {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
            connections.push(tokio::net::TcpStream::connect(address).await?);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Err("did not observe real accept EMFILE".into())
    })
    .await??;
    let exhausted_at = Instant::now();
    assert_eq!(
        std::fs::read_dir(format!("/proc/{}/fd", child.0.id()))?.count(),
        64
    );
    // Existing work/probes keep running; resource pressure alone never starts DRAINING.
    let version: serde_json::Value = client
        .get(format!("http://{operations}/version"))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(version["state"], "READY");
    let response = client
        .get(format!("http://{operations}/ready"))
        .send()
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    response.bytes().await?;
    let response = client.get(format!("http://{api}/fast")).send().await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    response.bytes().await?;
    timeout(Duration::from_secs(3), async {
        while fd_retry_count(&directory)? < 2 {
            assert!(
                child.0.try_wait()?.is_none(),
                "child exited while retrying accept"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await??;
    assert!(
        exhausted_at.elapsed() >= Duration::from_millis(750),
        "accept busy-looped instead of backing off"
    );
    assert_eq!(fd_retry_count(&directory)?, 2);

    if recover {
        connections.clear();
        // A fresh client (not the reserved keep-alive) proves new accepts recover on both listeners.
        let fresh = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(4))
            .build()?;
        for url in [
            format!("http://{api}/fast"),
            format!("http://{operations}/ready"),
        ] {
            let response = fresh.get(url).send().await?;
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
            response.bytes().await?;
        }
        let version: serde_json::Value = fresh
            .get(format!("http://{operations}/version"))
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(version["state"], "READY");
    }
    assert!(
        child.0.try_wait()?.is_none(),
        "child exited without a signal"
    );
    let stopping_at = Instant::now();
    child.signal(signal)?;
    assert!(child.wait(Duration::from_secs(2))?.success());
    assert!(stopping_at.elapsed() < Duration::from_secs(2));
    assert_eq!(
        std::fs::read_to_string(directory.path("stopped"))?,
        "joined"
    );
    assert_eq!(std::fs::read_to_string("/proc/self/limits")?, parent_limits);
    // In the non-recovery cases all pressure stays applied until after the child is reaped.
    drop(connections);
    Ok(())
}

#[tokio::test]
async fn should_fail_search_readiness_on_non_success_status_without_logging_provider_body()
-> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let router = Router::new().route(
        "/",
        get(|| async { (StatusCode::SERVICE_UNAVAILABLE, "provider_secret_canary") }),
    );
    let (stop, stop_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(serve(listener, router, async {
        let _closed = stop_rx.await;
    }));
    let mut values = inputs();
    values.insert("OPENSEARCH_ENDPOINT_URL", format!("http://{address}"));
    let config = parse(&values)?;
    let error = check_search(&config.opensearch)
        .await
        .err()
        .ok_or("expected dependency failure")?;
    assert!(!format!("{error:?} {error}").contains("canary"));
    stop.send(()).map_err(|_| "server stopped unexpectedly")?;
    tokio::time::timeout(Duration::from_secs(3), server).await???;
    Ok(())
}

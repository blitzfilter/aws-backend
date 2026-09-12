//! Real binary boundary. Local HTTP fixtures are protocol witnesses, not cloud-service proofs.
#[path = "../src/local_postgres_fixture.rs"]
mod local_postgres_fixture;
#[path = "../src/runtime_test_support.rs"]
mod support;

use application::transaction::{Transaction, UnitOfWork};
use axum::{
    Router,
    extract::Request,
    http::{Method, StatusCode},
    response::IntoResponse,
};
use std::{
    collections::BTreeMap,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use support::{OwnedChild, OwnedDirectory, TestResult, inputs, wait_for_file};
use tokio::{net::TcpListener, task::JoinSet};
use user_core::{
    access_token::{
        AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken,
        RawAccessToken, Scope,
    },
    user_id::UserId,
};
use user_service::ports::{AccessTokenRepository, AccessTokenRepositoryFactory};

fn binary(
    values: &BTreeMap<&'static str, String>,
    args: &[&str],
    directory: &OwnedDirectory,
) -> TestResult<OwnedChild> {
    let log = std::fs::File::create(directory.path("output"))?;
    let child = Command::new(env!("CARGO_BIN_EXE_aura-historia-api"))
        .args(args)
        .env_clear()
        .env("HOME", &directory.0)
        .env("AWS_CONFIG_FILE", directory.path("no-aws-config"))
        .env(
            "AWS_SHARED_CREDENTIALS_FILE",
            directory.path("no-aws-credentials"),
        )
        .env(
            "GOOGLE_APPLICATION_CREDENTIALS",
            directory.path("no-google-credentials"),
        )
        .envs(values)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    Ok(OwnedChild(child))
}

fn check_output(directory: &OwnedDirectory) -> TestResult {
    let output = std::fs::read_to_string(directory.path("output"))?;
    for sensitive in [
        "canary",
        "client_secret",
        "refresh_token",
        "provider_secret",
        "password_canary",
    ] {
        assert!(
            !output.contains(sensitive),
            "process output exposed sensitive input"
        );
    }
    Ok(())
}

fn unused_address() -> TestResult<std::net::SocketAddr> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?)
}

#[tokio::test]
async fn should_reject_arguments_and_config_before_any_endpoint_or_cloud_discovery() -> TestResult {
    let witness = std::net::TcpListener::bind("127.0.0.1:0")?;
    witness.set_nonblocking(true)?;
    let address = witness.local_addr()?;
    for (key, value) in [
        ("STAGE", "unknown"),
        ("STAGE", "dev"),
        ("STAGE", "prod"),
        ("COMMIT_SHA", "secret_canary"),
        ("POSTGRES_SSL_MODE", "verify-full"),
        ("AURA_HISTORIA_API_OPERATIONS_BIND_ADDR", "0.0.0.0:9080"),
        ("AURA_HISTORIA_API_DRAIN_SECONDS", "30"),
        ("OPENSEARCH_ENDPOINT_URL", "ftp://secret_canary.invalid"),
        ("STRIPE_API_KEY", ""),
    ] {
        for args in [vec![], vec!["--check-config"]] {
            let directory = OwnedDirectory::new()?;
            let mut values = inputs();
            values.insert("POSTGRES_PORT", address.port().to_string());
            values.insert(
                "AWS_EC2_METADATA_SERVICE_ENDPOINT",
                format!("http://{address}"),
            );
            values.insert("GCE_METADATA_HOST", address.to_string());
            values.insert(key, value.into());
            if key == "STAGE" && matches!(value, "dev" | "prod") {
                values.insert("POSTGRES_SSL_MODE", "verify-full".into());
                values.insert(
                    "COMMIT_SHA",
                    "d5bd9ca854e713b0c587528f02037211b2020fd4".into(),
                );
            }
            let mut child = binary(&values, &args, &directory)?;
            assert!(!child.wait(Duration::from_secs(4))?.success());
            wait_for_file(&directory.path("output")).await?;
            check_output(&directory)?;
            assert!(
                matches!(witness.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
    }
    let directory = OwnedDirectory::new()?;
    let mut child = binary(
        &BTreeMap::new(),
        &["--migrate", "secret_canary"],
        &directory,
    )?;
    assert!(!child.wait(Duration::from_secs(2))?.success());
    check_output(&directory)?;
    Ok(())
}

#[tokio::test]
async fn should_fail_preflight_with_bounded_dependency_failure_and_no_listener() -> TestResult {
    // A held socket never replies to PostgreSQL startup. No live service is contacted.
    let unavailable = std::net::TcpListener::bind("127.0.0.1:0")?;
    let occupied_operations = std::net::TcpListener::bind("127.0.0.1:0")?;
    let directory = OwnedDirectory::new()?;
    let mut values = inputs();
    values.insert(
        "POSTGRES_PORT",
        unavailable.local_addr()?.port().to_string(),
    );
    values.insert(
        "AURA_HISTORIA_API_OPERATIONS_BIND_ADDR",
        occupied_operations.local_addr()?.to_string(),
    );
    let mut child = binary(&values, &["--check-config"], &directory)?;
    assert!(!child.wait(Duration::from_secs(8))?.success());
    check_output(&directory)?;
    Ok(())
}

#[rstest::rstest]
#[case("-INT")]
#[case("-TERM")]
#[tokio::test]
async fn should_handle_both_signals_during_real_binary_startup(#[case] signal: &str) -> TestResult {
    let unavailable = std::net::TcpListener::bind("127.0.0.1:0")?;
    let operations = unused_address()?;
    let directory = OwnedDirectory::new()?;
    let mut values = inputs();
    values.insert(
        "POSTGRES_PORT",
        unavailable.local_addr()?.port().to_string(),
    );
    values.insert(
        "AURA_HISTORIA_API_OPERATIONS_BIND_ADDR",
        operations.to_string(),
    );
    let mut child = binary(&values, &[], &directory)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()?;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(response) = client
                .get(format!("http://{operations}/version"))
                .send()
                .await
            {
                assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-store");
                assert_eq!(
                    response.json::<serde_json::Value>().await?["state"],
                    "STARTING"
                );
                return Ok::<_, reqwest::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(
        client
            .get(format!("http://{operations}/ready"))
            .send()
            .await?
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    child.signal(signal)?;
    assert!(child.wait(Duration::from_secs(3))?.success());
    check_output(&directory)?;
    Ok(())
}

struct EndpointWitness {
    address: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<(Method, String)>>>,
    healthy: Arc<AtomicBool>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    tasks: JoinSet<Result<(), aura_historia_api::ApiRunError>>,
}

impl EndpointWitness {
    async fn start() -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let healthy = Arc::new(AtomicBool::new(true));
        let request_log = requests.clone();
        let health = healthy.clone();
        let router = Router::new().fallback(move |request: Request| {
            let requests = request_log.clone();
            let healthy = health.load(Ordering::SeqCst);
            async move {
                match requests.lock() {
                    Ok(mut requests) => requests.push((request.method().clone(), request.uri().path().to_owned())),
                    Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                }
                match (request.method(), request.uri().path(), healthy) {
                    (&Method::HEAD, "/", true) => StatusCode::OK.into_response(),
                    (&Method::GET, "/jwks", _) => axum::Json(serde_json::json!({"keys":[{"kid":"test-key", "alg":"RS256", "n":"AQAB", "e":"AQAB"}]})).into_response(),
                    _ => (StatusCode::SERVICE_UNAVAILABLE, "provider_secret_canary").into_response(),
                }
            }
        });
        let (stop, receiver) = tokio::sync::oneshot::channel();
        let mut tasks = JoinSet::new();
        tasks.spawn(aura_historia_api::serve(listener, router, async {
            let _closed = receiver.await;
        }));
        Ok(Self {
            address,
            requests,
            healthy,
            stop: Some(stop),
            tasks,
        })
    }

    fn assert_allowed_calls(&self, normal_provider_startup: bool) -> TestResult {
        let requests = self.requests.lock().map_err(|_| "witness lock poisoned")?;
        assert!(!requests.is_empty());
        let unexpected = requests.iter().find(|(method, path)| {
            !((method == Method::HEAD && path == "/")
                || (method == Method::GET && path == "/jwks")
                || (normal_provider_startup && method == Method::POST && path == "/oauth-token"))
        });
        assert!(
            unexpected.is_none(),
            "unexpected local witness method/path: {unexpected:?}"
        );
        Ok(())
    }

    fn reset(&self) -> TestResult {
        self.requests
            .lock()
            .map_err(|_| "witness lock poisoned")?
            .clear();
        Ok(())
    }

    async fn close(mut self) -> TestResult {
        if let Some(stop) = self.stop.take() {
            stop.send(()).map_err(|_| "witness stopped early")?;
        }
        while let Some(result) =
            tokio::time::timeout(Duration::from_secs(3), self.tasks.join_next()).await?
        {
            result??;
        }
        Ok(())
    }
}

// Same user/token construction as api_cases/users.rs and api_support, but only this owned DB.
async fn seed_profile_writer(pool: &sqlx::PgPool) -> TestResult<(UserId, String)> {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(user_id.as_uuid())
        .bind(format!("{}@example.test", user_id.as_uuid()))
        .execute(pool)
        .await
        .map_err(|_| "isolated user seed failed")?;
    let raw = RawAccessToken::new();
    let token = AccessToken::create(NewAccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw.clone().into(),
        user_id,
        name: AccessTokenName::from("api acceptance"),
        scopes: [Scope::UsersWrite].into(),
        origin: AccessTokenOrigin::User,
        expires: None,
    });
    let mut tx = platform_postgres::SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await?;
    user_postgres::SqlxAccessTokenRepositoryFactory::new()
        .in_transaction(&mut tx)
        .insert(&token)
        .await
        .map_err(|_| "isolated access-token seed failed")?;
    tx.commit().await?;
    Ok((user_id, String::from(raw)))
}

async fn profile_snapshot(pool: &sqlx::PgPool, user_id: UserId) -> TestResult<serde_json::Value> {
    Ok(
        sqlx::query_scalar("SELECT to_jsonb(users) FROM users WHERE user_id = $1")
            .bind(user_id.as_uuid())
            .fetch_one(pool)
            .await?,
    )
}

async fn wait_for_profile_write(pool: &sqlx::PgPool, holder_pid: i32) -> TestResult<i32> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            // AFTER UPDATE trigger: authenticated business SQL has staged a row, not just
            // accepted a socket or entered auth. Only our barrier connection can block it.
            let pid = sqlx::query_scalar::<_, i32>(
                "SELECT pid FROM pg_stat_activity
                 WHERE datname = current_database() AND usename = 'api_lifecycle'
                   AND application_name = 'aura-historia-api' AND state = 'active'
                   AND wait_event_type = 'Lock' AND wait_event = 'advisory'
                   AND query LIKE '%UPDATE users SET%'
                   AND $1 = ANY(pg_blocking_pids(pid))",
            )
            .bind(holder_pid)
            .fetch_optional(pool)
            .await?;
            if let Some(pid) = pid {
                return Ok(pid);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

async fn prove_profile_write_drain(
    pool: &sqlx::PgPool,
    client: &reqwest::Client,
    addresses: (std::net::SocketAddr, std::net::SocketAddr),
    child: &mut OwnedChild,
    signal: &str,
    directory: &OwnedDirectory,
) -> TestResult {
    let (public, operations) = addresses;
    let (user_id, token) = seed_profile_writer(pool).await?;
    let before = profile_snapshot(pool, user_id).await?;
    let original_version = before["version"].as_i64().ok_or("missing user version")?;
    let body = serde_json::json!({"firstName": "profile_write_canary"});
    let url = format!("http://{public}/api/v1/me/account");
    for credential in [None, Some("invalid_canary")] {
        let mut request = client.patch(&url).json(&body);
        if let Some(credential) = credential {
            request = request.bearer_auth(credential);
        }
        assert_eq!(request.send().await?.status(), StatusCode::UNAUTHORIZED);
    }
    assert!(
        profile_snapshot(pool, user_id).await? == before,
        "unauthorized write changed user"
    );

    // Cancel once, then retry the identical command/token through the real HTTP boundary.
    for interrupted in [true, false] {
        let mut barrier = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(1341, 3)")
            .execute(&mut *barrier)
            .await?;
        let holder_pid = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
            .fetch_one(&mut *barrier)
            .await?;
        let correlation = format!("cli-write-{}-{interrupted}", signal.trim_start_matches('-'));
        let response = client
            .patch(&url)
            .bearer_auth(&token)
            .header("x-correlation-id", &correlation)
            .json(&body)
            .timeout(Duration::from_secs(8))
            .send();
        tokio::pin!(response);
        let writer_pid = tokio::select! {
            biased;
            _ = &mut response => return Err("write completed before database barrier".into()),
            result = wait_for_profile_write(pool, holder_pid) => result?,
        };
        assert!(
            profile_snapshot(pool, user_id).await? == before,
            "blocked write became visible"
        );
        let shutdown_started = Instant::now();
        let barrier = if interrupted {
            // Target only the witnessed backend in this newly created fixture, never ambient PG.
            assert!(
                sqlx::query_scalar::<_, bool>(
                    "SELECT pg_cancel_backend(pid) FROM pg_stat_activity
                 WHERE pid = $1 AND usename = 'api_lifecycle'
                   AND application_name = 'aura-historia-api'
                   AND $2 = ANY(pg_blocking_pids(pid))",
                )
                .bind(writer_pid)
                .bind(holder_pid)
                .fetch_one(pool)
                .await?
            );
            Some(barrier)
        } else {
            child.signal(signal)?;
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let version = client
                        .get(format!("http://{operations}/version"))
                        .send()
                        .await?
                        .json::<serde_json::Value>()
                        .await?;
                    if version["state"] == "DRAINING" {
                        return Ok::<_, reqwest::Error>(());
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await??;
            assert_eq!(
                client
                    .get(format!("http://{operations}/ready"))
                    .send()
                    .await?
                    .status(),
                StatusCode::SERVICE_UNAVAILABLE
            );
            assert!(
                child.0.try_wait()?.is_none(),
                "process exited with accepted write blocked"
            );
            assert_eq!(wait_for_profile_write(pool, holder_pid).await?, writer_pid);
            assert!(
                profile_snapshot(pool, user_id).await? == before,
                "draining write committed before release"
            );
            barrier.rollback().await?;
            None
        };
        let response = tokio::time::timeout(Duration::from_secs(3), &mut response).await??;
        assert_eq!(response.headers()["x-correlation-id"], correlation);
        uuid::Uuid::parse_str(response.headers()["x-request-id"].to_str()?)?;
        let status = response.status();
        if !interrupted {
            assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-store");
        }
        let response_body = response.json::<serde_json::Value>().await?;
        if let Some(barrier) = barrier {
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            // A row-locking read cannot finish until the failed writer has rolled back.
            tokio::time::timeout(
                Duration::from_secs(2),
                sqlx::query("SELECT user_id FROM users WHERE user_id = $1 FOR UPDATE")
                    .bind(user_id.as_uuid())
                    .fetch_one(pool),
            )
            .await??;
            assert!(
                profile_snapshot(pool, user_id).await? == before,
                "cancelled write was not rolled back"
            );
            barrier.rollback().await?;
            eprintln!(
                "business-write {signal}: DB cancellation -> HTTP 503; whole user row unchanged; row lock released"
            );
        } else {
            assert_eq!(status, StatusCode::OK);
            assert!(
                response_body["firstName"] == body["firstName"],
                "write response did not contain changed profile"
            );
            assert!(
                response_body["userId"] == user_id.to_string(),
                "write response targeted wrong user"
            );
            let committed = profile_snapshot(pool, user_id).await?;
            assert!(
                committed["first_name"] == body["firstName"],
                "profile effect not committed"
            );
            assert_eq!(committed["version"].as_i64(), Some(original_version + 1));
            assert!(child.wait(Duration::from_secs(4))?.success());
            assert!(shutdown_started.elapsed() < Duration::from_secs(8));
            assert!(
                profile_snapshot(pool, user_id).await? == committed,
                "committed profile did not survive process exit"
            );
            assert_eq!(sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM pg_stat_activity WHERE application_name = 'aura-historia-api'",
            ).fetch_one(pool).await?, 0);
            for address in [public, operations] {
                assert!(
                    std::net::TcpListener::bind(address).is_ok(),
                    "API listener not released"
                );
            }
            eprintln!(
                "business-write {signal}: staged UPDATE -> DRAINING/ready=503 -> barrier release -> HTTP 200; committed version +1 survives exit; shutdown={}ms, exit=0, API sessions=0, listeners released",
                shutdown_started.elapsed().as_millis()
            );
        }
    }
    assert!(
        !std::fs::read_to_string(directory.path("output"))?.contains(&token),
        "process output exposed access token"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "opt-in: owned local PostgreSQL container, cached image only"]
async fn should_preflight_read_only_and_boot_real_binary_without_paid_calls() -> TestResult {
    let postgres = local_postgres_fixture::LocalPostgres::start().await?;
    let witness = EndpointWitness::start().await?;
    let mut values = inputs();
    values.insert("POSTGRES_PORT", postgres.port.to_string());
    values.insert("POSTGRES_USERNAME", "api_preflight".into());
    let endpoint = format!("http://{}", witness.address);
    values.insert("OPENSEARCH_ENDPOINT_URL", endpoint.clone());
    values.insert("AURA_HISTORIA_COGNITO_JWKS_URL", format!("{endpoint}/jwks"));
    values.insert("ZOHO_ACCOUNTS_URL", endpoint.clone());
    values.insert("ZOHO_CAMPAIGNS_URL", endpoint.clone());
    values.insert("AWS_ENDPOINT_URL", endpoint.clone());
    values.insert("HTTP_PROXY", endpoint.clone());
    values.insert("HTTPS_PROXY", endpoint.clone());
    values.insert("NO_PROXY", "127.0.0.1,localhost".into());
    // Occupied ports prove preflight does not bind. Missing ADC proves it skips discovery.
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    values.insert(
        "AURA_HISTORIA_API_BIND_ADDR",
        occupied.local_addr()?.to_string(),
    );
    values.insert(
        "AURA_HISTORIA_API_OPERATIONS_BIND_ADDR",
        occupied.local_addr()?.to_string(),
    );
    let directory = OwnedDirectory::new()?;
    let mut child = binary(&values, &["--check-config"], &directory)?;
    assert!(child.wait(Duration::from_secs(15))?.success());
    check_output(&directory)?;
    witness.assert_allowed_calls(false)?;
    let before: String = sqlx::query_scalar("SELECT value FROM public.preflight_canary")
        .fetch_one(&postgres.admin)
        .await?;
    assert_eq!(before, "unchanged");

    // Full production composition with a narrowly granted fixture writer, never live data.
    sqlx::raw_sql(
        "CREATE ROLE api_lifecycle LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
         GRANT USAGE ON SCHEMA public TO api_lifecycle;
         GRANT SELECT ON public._sqlx_migrations, public.users, public.access_tokens TO api_lifecycle;
         GRANT UPDATE ON public.users TO api_lifecycle;
         CREATE FUNCTION public.cli_profile_write_barrier() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_advisory_xact_lock(1341, 3);
             RETURN NEW;
         END $$;
         CREATE TRIGGER cli_profile_write_barrier AFTER UPDATE ON public.users
             FOR EACH ROW WHEN (OLD.first_name IS DISTINCT FROM NEW.first_name)
             EXECUTE FUNCTION public.cli_profile_write_barrier();",
    )
    .execute(&postgres.admin)
    .await?;
    values.insert("POSTGRES_USERNAME", "api_lifecycle".into());
    // Google auth eagerly refreshes tokens. Its FREE authentication endpoint is a local
    // failing witness, not an inference/paid endpoint; no real credentials or tokens are used.
    // Preflight above and below must never make even this authentication request.
    let adc = directory.path("fixture-adc.json");
    std::fs::write(
        &adc,
        serde_json::to_vec(&serde_json::json!({
            "type":"authorized_user", "client_id":"local-test", "client_secret":"local-test",
            "refresh_token":"local-test", "token_uri":format!("{endpoint}/oauth-token")
        }))?,
    )?;
    values.insert(
        "GOOGLE_APPLICATION_CREDENTIALS",
        adc.to_string_lossy().into_owned(),
    );
    for signal in ["-INT", "-TERM"] {
        let operations = unused_address()?;
        let public = unused_address()?;
        values.insert(
            "AURA_HISTORIA_API_OPERATIONS_BIND_ADDR",
            operations.to_string(),
        );
        values.insert("AURA_HISTORIA_API_BIND_ADDR", public.to_string());
        let mut child = binary(&values, &[], &directory)?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()?;
        tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                if let Ok(response) = client
                    .get(format!("http://{operations}/ready"))
                    .send()
                    .await
                    && response.status() == StatusCode::NO_CONTENT
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        for path in ["health", "ready", "version"] {
            assert_eq!(
                client
                    .get(format!("http://{public}/{path}"))
                    .send()
                    .await?
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        let response = client
            .get(format!("http://{operations}/version"))
            .send()
            .await?;
        assert_eq!(response.headers()[http::header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.json::<serde_json::Value>().await?,
            serde_json::json!({"commit_sha":"unversioned", "state":"READY"})
        );
        if signal == "-INT" {
            for (healthy, expected) in [
                (false, StatusCode::SERVICE_UNAVAILABLE),
                (true, StatusCode::NO_CONTENT),
            ] {
                witness.healthy.store(healthy, Ordering::SeqCst);
                tokio::time::timeout(Duration::from_secs(12), async {
                    loop {
                        let response = client
                            .get(format!("http://{operations}/ready"))
                            .send()
                            .await?;
                        if response.status() == expected {
                            return Ok::<_, reqwest::Error>(());
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                })
                .await??;
            }
        }
        prove_profile_write_drain(
            &postgres.admin,
            &client,
            (public, operations),
            &mut child,
            signal,
            &directory,
        )
        .await?;
        check_output(&directory)?;
    }
    values.insert("POSTGRES_USERNAME", "api_preflight".into());
    witness.assert_allowed_calls(true)?;
    witness.reset()?;
    // Even with valid ADC construction inputs, preflight must not start SDK refresh tasks.
    let mut child = binary(&values, &["--check-config"], &directory)?;
    assert!(child.wait(Duration::from_secs(15))?.success());
    witness.assert_allowed_calls(false)?;
    check_output(&directory)?;
    // Endpoint failures and dirty schema fail both preflight and normal startup, never repair.
    witness.healthy.store(false, Ordering::SeqCst);
    for args in [vec!["--check-config"], vec![]] {
        let mut child = binary(&values, &args, &directory)?;
        assert!(!child.wait(Duration::from_secs(12))?.success());
        check_output(&directory)?;
    }
    witness.healthy.store(true, Ordering::SeqCst);
    sqlx::query("UPDATE public._sqlx_migrations SET success = false WHERE version = (SELECT min(version) FROM public._sqlx_migrations)").execute(&postgres.admin).await?;
    for args in [vec!["--check-config"], vec![]] {
        let mut child = binary(&values, &args, &directory)?;
        assert!(!child.wait(Duration::from_secs(12))?.success());
        check_output(&directory)?;
    }
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM public.preflight_canary")
            .fetch_one(&postgres.admin)
            .await?,
        before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM public._sqlx_migrations WHERE NOT success"
        )
        .fetch_one(&postgres.admin)
        .await?,
        1
    );
    witness.assert_allowed_calls(false)?;
    let witness_address = witness.address;
    witness.close().await?;
    assert!(
        std::net::TcpListener::bind(witness_address).is_ok(),
        "witness listener not released"
    );
    let postgres_address = ("127.0.0.1", postgres.port);
    postgres.close().await?;
    assert!(
        std::net::TcpListener::bind(postgres_address).is_ok(),
        "owned PostgreSQL port not released"
    );
    let directory_path = directory.0.clone();
    drop(directory);
    assert!(
        !directory_path.exists(),
        "owned API test directory not removed"
    );
    eprintln!(
        "cleanup: witness joined; PostgreSQL pool closed and owned container removed; ports and temporary directories released"
    );
    Ok(())
}

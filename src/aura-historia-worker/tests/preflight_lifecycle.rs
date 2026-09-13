//! Actual worker processes and real PostgreSQL; loopback SQS protocol spies, not SQS durability proof.
#[allow(dead_code)]
mod process_support;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
};
use process_support::*;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use tokio::{net::TcpListener, sync::Notify, task::JoinSet};

const POSTGRES: ProcessPostgres = ProcessPostgres;
const SHA: &str = "d5bd9ca854e713b0c587528f02037211b2020fd4";

#[derive(Clone)]
struct SqsSpy {
    scope: &'static str,
    endpoint: String,
    operations: Arc<Mutex<Vec<String>>>,
    message: Arc<Mutex<Option<String>>>,
    sent: Arc<Notify>,
    fail_attributes: Option<bool>,
    fail_heartbeat: Arc<AtomicBool>,
    hold_attributes: Arc<AtomicBool>,
    release_attributes: Arc<Notify>,
}
impl SqsSpy {
    fn queue_url(&self) -> String {
        format!(
            "{}/000000000000/aura-worker-{}-test",
            self.endpoint, self.scope
        )
    }
    fn count(&self, operation: &str) -> usize {
        self.operations
            .lock()
            .unwrap()
            .iter()
            .filter(|actual| actual.as_str() == operation)
            .count()
    }
    fn assert_attributes_only(&self, expected: usize) {
        let operations = self.operations.lock().unwrap();
        assert_eq!(expected, operations.len());
        assert!(
            operations
                .iter()
                .all(|operation| operation == "GetQueueAttributes"),
            "preflight attempted queue custody operation"
        );
    }
    fn attributes(&self, dlq: bool) -> Value {
        let arn = |dlq: bool| {
            format!(
                "arn:aws:sqs:eu-central-1:000000000000:aura-worker-{}{}-test",
                self.scope,
                if dlq { "-dlq" } else { "" }
            )
        };
        let slow = matches!(
            self.scope,
            "notification-delivery"
                | "search-filter-percolator"
                | "product-embedding"
                | "product-translation"
                | "product-listing-normalization"
        );
        let mut attributes = json!({
            "QueueArn":arn(dlq), "FifoQueue":"false", "SqsManagedSseEnabled":"true",
            "MessageRetentionPeriod":if dlq {"1209600"} else {"604800"},
            "VisibilityTimeout":if self.scope == "notification-delivery" {"360"} else if slow {"300"} else {"60"},
            "ReceiveMessageWaitTimeSeconds":"20",
            "Policy":json!({"Statement":[{"Effect":"Deny","Principal":"*","Action":"sqs:*","Resource":arn(dlq),"Condition":{"Bool":{"aws:SecureTransport":"false"}}}]}).to_string(),
            "RedriveAllowPolicy":if dlq {json!({"redrivePermission":"byQueue","sourceQueueArns":[arn(false)]})} else {json!({"redrivePermission":"denyAll"})}.to_string(),
        });
        if !dlq {
            attributes["RedrivePolicy"] =
                json!({"deadLetterTargetArn":arn(true),"maxReceiveCount":5})
                    .to_string()
                    .into();
        }
        if self.fail_attributes == Some(dlq) {
            attributes["MessageRetentionPeriod"] = "1".into();
        }
        json!({"Attributes":attributes})
    }
}
struct SpyServer {
    spy: SqsSpy,
    tasks: JoinSet<()>,
}
impl SpyServer {
    async fn start(scope: &'static str, fail_attributes: Option<bool>) -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let spy = SqsSpy {
            scope,
            endpoint: format!("http://{}", listener.local_addr()?),
            operations: Arc::default(),
            message: Arc::default(),
            sent: Arc::default(),
            fail_attributes,
            fail_heartbeat: Arc::default(),
            hold_attributes: Arc::default(),
            release_attributes: Arc::default(),
        };
        let router = Router::new()
            .route("/", post(sqs_request))
            .with_state(spy.clone());
        let mut tasks = JoinSet::new();
        tasks.spawn(async move {
            assert!(
                axum::serve(listener, router).await.is_ok(),
                "loopback SQS spy failed"
            );
        });
        Ok(Self { spy, tasks })
    }
    async fn stop(mut self) {
        self.spy.release_attributes.notify_waiters();
        self.tasks.shutdown().await;
    }
}
async fn sqs_request(
    State(spy): State<SqsSpy>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> axum::response::Response {
    // AWS JSON uses application/x-amz-json-1.0, not Axum Json's media type.
    let request: Value = serde_json::from_slice(&body).expect("valid SDK JSON request");
    let operation = headers
        .get("x-amz-target")
        .and_then(|value| value.to_str().ok())
        .and_then(|target| target.rsplit('.').next())
        .unwrap_or("unknown");
    spy.operations.lock().unwrap().push(operation.into());
    if operation == "GetQueueAttributes" && spy.hold_attributes.load(Ordering::SeqCst) {
        spy.release_attributes.notified().await;
    }
    match operation {
        "GetQueueAttributes" => Json(
            spy.attributes(
                request["QueueUrl"]
                    .as_str()
                    .is_some_and(|url| url.ends_with("-dlq-test")),
            ),
        )
        .into_response(),
        "SendMessage" => {
            *spy.message.lock().unwrap() = request["MessageBody"].as_str().map(str::to_owned);
            spy.sent.notify_one();
            Json(json!({"MessageId":"local-message"})).into_response()
        }
        "ReceiveMessage" => {
            if spy.message.lock().unwrap().is_none() {
                let _timeout =
                    tokio::time::timeout(Duration::from_secs(20), spy.sent.notified()).await;
            }
            let body = spy.message.lock().unwrap().take();
            Json(match body {
                Some(body) => json!({"Messages":[{"MessageId":"local-message","ReceiptHandle":"local-private-receipt","Body":body,
                    "Attributes":{"ApproximateReceiveCount":"1","SentTimestamp":"0","ApproximateFirstReceiveTimestamp":"0"}}]}),
                None => json!({}),
            }).into_response()
        }
        "ChangeMessageVisibility" if spy.fail_heartbeat.load(Ordering::SeqCst) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"__type":"ReceiptHandleIsInvalid","message":"redacted fixture failure"})),
        )
            .into_response(),
        "ChangeMessageVisibility" | "DeleteMessage" => Json(json!({})).into_response(),
        _ => (StatusCode::BAD_REQUEST, Json(json!({}))).into_response(),
    }
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_preflight_all_ten_scopes_with_only_schema_and_attributes_without_provider_auth_or_business_access()
 {
    case(async {
        let pool = get_postgres_client().await;
        // This role cannot invoke normalizer repair or any domain handler read/write.
        // Success therefore needs only the schema gate's documented read permissions.
        let role = format!("worker_preflight_{}", uuid::Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE ROLE {role} LOGIN PASSWORD 'local-test-only'; ALTER ROLE {role} SET default_transaction_read_only=on; GRANT USAGE ON SCHEMA public TO {role}; GRANT SELECT ON public._sqlx_migrations TO {role};"))).execute(&pool).await?;
        let search_listener = TcpListener::bind("127.0.0.1:0").await?;
        let search_endpoint = format!("http://{}", search_listener.local_addr()?);
        let search_calls = Arc::new(Mutex::new(Vec::new()));
        let calls = search_calls.clone();
        let search = Router::new().fallback(move |request: axum::extract::Request| {
            let calls = calls.clone();
            async move {
                calls.lock().unwrap().push((request.method().clone(), request.uri().path().to_owned()));
                Json(json!({"version":{"distribution":"opensearch","number":"3.1.0"}}))
            }
        });
        let mut tasks = JoinSet::new();
        tasks.spawn(async move { assert!(axum::serve(search_listener, search).await.is_ok()); });
        for scope in aura_historia_worker::WorkerScope::iter() {
            let server = SpyServer::start(scope.as_str(), None).await?;
            let address = unused_address()?;
            let overrides = [
                ("AURA_HISTORIA_WORKER_SCOPE", scope.as_str()), ("POSTGRES_USERNAME", role.as_str()),
                ("POSTGRES_PASSWORD", "local-test-only"), ("COMMIT_SHA", SHA),
                ("OPENSEARCH_ENDPOINT_URL", search_endpoint.as_str()),
                ("VERTEX_AI_PROJECT_ID", "preflight-local"), ("VERTEX_AI_LOCATION", "europe-west3"),
                ("VERTEX_AI_MODEL", "preflight-model"), ("GOOGLE_APPLICATION_CREDENTIALS", "/nonexistent/preflight-no-auth"),
                ("S3_BUCKET_NAME_TEMPLATES", "local-templates"), ("NOTIFICATION_EMAIL_FROM", "sender@example.test"),
                ("NOTIFICATION_EMAIL_REPLY_TO", "reply@example.test"),
            ];
            let mut child = WorkerProcess::spawn(&pool, &server.spy.endpoint, &server.spy.queue_url(), address, &overrides, true)?;
            child.wait_for_clean_exit().await?;
            server.spy.assert_attributes_only(2);
            assert!(tokio::net::TcpStream::connect(address).await.is_err(), "preflight bound the worker listener");
            server.stop().await;
        }
        {
            let calls = search_calls.lock().unwrap();
            assert_eq!(3, calls.len());
            assert!(calls.iter().all(|(method, path)| method == "GET" && path == "/"));
        }
        assert_eq!(0, assessment_count(&pool).await?);
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM product_listing_raw_normalizations").fetch_one(&pool).await?;
        assert_eq!(0, rows);
        tasks.shutdown().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP OWNED BY {role}; DROP ROLE {role};"))).execute(&pool).await?;
        Ok(())
    }).await;
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_fail_preflight_closed_for_invalid_source_dlq_and_full_scoped_config_without_custody_operations()
 {
    case(async {
        let pool = get_postgres_client().await;
        for (bad_attributes, invalid_config, calls) in [
            (Some(false), false, 1),
            (Some(true), false, 2),
            (None, true, 0),
        ] {
            let server = SpyServer::start("product-content-assessment", bad_attributes).await?;
            let overrides = if invalid_config {
                vec![("AURA_HISTORIA_WORKER_DRAIN_TIMEOUT_SECONDS", "0")]
            } else {
                vec![]
            };
            let mut child = WorkerProcess::spawn(
                &pool,
                &server.spy.endpoint,
                &server.spy.queue_url(),
                unused_address()?,
                &overrides,
                true,
            )?;
            child.wait_for_exit(1).await?;
            server.spy.assert_attributes_only(calls);
            server.stop().await;
        }
        // A fresh uninitialized database must not be migrated or stamped by startup/preflight.
        let database = format!("worker_preflight_{}", uuid::Uuid::new_v4().simple());
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE {database}")))
            .execute(&pool)
            .await?;
        let empty = sqlx::postgres::PgPoolOptions::new()
            .connect_with(pool.connect_options().as_ref().clone().database(&database))
            .await?;
        let server = SpyServer::start("product-content-assessment", None).await?;
        for check_config in [true, false] {
            let mut child = WorkerProcess::spawn(
                &empty,
                &server.spy.endpoint,
                &server.spy.queue_url(),
                unused_address()?,
                &[],
                check_config,
            )?;
            child.wait_for_exit(1).await?;
            server.spy.assert_attributes_only(0);
        }
        let tables: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_tables WHERE schemaname='public'")
                .fetch_one(&empty)
                .await?;
        assert_eq!(0, tables);
        empty.close().await;
        server.stop().await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP DATABASE {database}")))
            .execute(&pool)
            .await?;
        Ok(())
    })
    .await;
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_register_both_signals_before_read_only_startup_and_never_report_interrupted_preflight_success()
 {
    case(async {
        let pool = get_postgres_client().await;
        for interrupt in [true, false] {
            for check_config in [true, false] {
                let server = SpyServer::start("product-content-assessment", None).await?;
                server.spy.hold_attributes.store(true, Ordering::SeqCst);
                let address = unused_address()?;
                let mut child = WorkerProcess::spawn(
                    &pool,
                    &server.spy.endpoint,
                    &server.spy.queue_url(),
                    address,
                    &[],
                    check_config,
                )?;
                eventually("first read-only startup request", async {
                    while server.spy.count("GetQueueAttributes") == 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Ok(())
                })
                .await?;
                if interrupt {
                    child.interrupt()?;
                } else {
                    child.terminate()?;
                }
                child
                    .wait_for_exit(if check_config { 1 } else { 0 })
                    .await?;
                server.spy.assert_attributes_only(1);
                assert!(tokio::net::TcpStream::connect(address).await.is_err());
                server.stop().await;
            }
        }
        Ok(())
    })
    .await;
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_incompatible_or_redirecting_search_endpoint_without_following_or_consuming()
{
    case(async {
        let pool = get_postgres_client().await;
        for status in [StatusCode::OK, StatusCode::FOUND, StatusCode::UNAUTHORIZED] {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let endpoint = format!("http://{}", listener.local_addr()?);
            let calls = Arc::new(Mutex::new(Vec::new()));
            let captured = calls.clone();
            let router = Router::new().fallback(move |request: axum::extract::Request| {
                let calls = captured.clone();
                async move {
                    calls.lock().unwrap().push(request.uri().path().to_owned());
                    (
                        status,
                        [("location", "/must-not-follow")],
                        Json(json!({"version":{"distribution":"opensearch","number":"4.0.0"}})),
                    )
                }
            });
            let mut tasks = JoinSet::new();
            tasks.spawn(async move {
                assert!(axum::serve(listener, router).await.is_ok());
            });
            let server = SpyServer::start("search-filter-projection", None).await?;
            let mut child = WorkerProcess::spawn(
                &pool,
                &server.spy.endpoint,
                &server.spy.queue_url(),
                unused_address()?,
                &[
                    ("AURA_HISTORIA_WORKER_SCOPE", "search-filter-projection"),
                    ("OPENSEARCH_ENDPOINT_URL", endpoint.as_str()),
                ],
                true,
            )?;
            child.wait_for_exit(1).await?;
            server.spy.assert_attributes_only(2);
            assert_eq!(vec!["/"], *calls.lock().unwrap());
            tasks.shutdown().await;
            server.stop().await;
        }
        Ok(())
    })
    .await;
}

async fn signal_attempt(
    pool: &sqlx::PgPool,
    interrupt: bool,
    deadline: bool,
    lost_heartbeat: bool,
) -> TestResult {
    let server = SpyServer::start("product-content-assessment", None).await?;
    let address = unused_address()?;
    let overrides = if deadline {
        vec![("AURA_HISTORIA_WORKER_DRAIN_TIMEOUT_SECONDS", "1")]
    } else {
        vec![]
    };
    let mut child = WorkerProcess::spawn(
        pool,
        &server.spy.endpoint,
        &server.spy.queue_url(),
        address,
        &overrides,
        false,
    )?;
    child.wait_for_ready(address).await?;
    let mut barrier = pool.begin().await?;
    sqlx::query("LOCK TABLE product_listing_content_assessments IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *barrier)
        .await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await?;
    let source = commit_source(pool).await?;
    let record: Value =
        sqlx::query_scalar("SELECT to_jsonb(e) FROM product_listing_events e WHERE event_id=$1")
            .bind(uuid::Uuid::from(source.event_id))
            .fetch_one(pool)
            .await?;
    let response = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(12)).build()?
        .post(format!("http://{address}/cdc/sequin")).json(&json!({"changes":[{"schema":"public","table":"product_listing_events","operation":"insert","record":record}]})).send().await?;
    assert_eq!(202, response.status());
    wait_for_blocked_handler(pool, blocker).await?;
    if interrupt {
        child.interrupt()?;
    } else {
        child.terminate()?;
    }
    wait_for_http_shutdown(address).await?;
    let receives = server.spy.count("ReceiveMessage");
    if !deadline {
        server
            .spy
            .fail_heartbeat
            .store(lost_heartbeat, Ordering::SeqCst);
        eventually("active-attempt heartbeat during signal drain", async {
            while server.spy.count("ChangeMessageVisibility") == 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(())
        })
        .await?;
    }
    if deadline || lost_heartbeat {
        child.wait_for_exit(if deadline { 1 } else { 0 }).await?;
        assert_eq!(0, server.spy.count("DeleteMessage"));
        barrier.rollback().await?;
        assert!(assessment(pool, source).await?.is_none());
    } else {
        child.assert_running()?;
        barrier.commit().await?;
        child.wait_for_clean_exit().await?;
        assert!(assessment(pool, source).await?.is_some());
        assert_eq!(1, server.spy.count("DeleteMessage"));
    }
    assert_eq!(
        receives,
        server.spy.count("ReceiveMessage"),
        "new receive after signal drain"
    );
    server.stop().await;
    Ok(())
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_drain_real_worker_on_sigint_and_sigterm_with_active_heartbeats_and_confirmed_terminal_delete()
 {
    case(async {
        let pool = get_postgres_client().await;
        for interrupt in [true, false] {
            signal_attempt(&pool, interrupt, false, false).await?;
        }
        Ok(())
    })
    .await;
}
#[aura_integration_test(services = [POSTGRES])]
async fn should_exit_nonzero_at_signal_drain_deadline_without_acknowledging_unfinished_receipt() {
    case(async {
        let pool = get_postgres_client().await;
        for interrupt in [true, false] {
            signal_attempt(&pool, interrupt, true, false).await?;
        }
        Ok(())
    })
    .await;
}
#[aura_integration_test(services = [POSTGRES])]
async fn should_never_ack_lost_receipt_when_heartbeat_fails_during_real_process_signal_drain() {
    case(async {
        let pool = get_postgres_client().await;
        signal_attempt(&pool, true, false, true).await
    })
    .await;
}

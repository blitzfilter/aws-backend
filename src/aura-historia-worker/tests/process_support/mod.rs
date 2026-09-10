#[allow(dead_code)]
mod relay;
pub use relay::{Observations, Receipt, Relay};

use aws_sdk_sqs::types::{Message, MessageSystemAttributeName, QueueAttributeName};
use domain_primitives::event_id::EventId;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use relay::Observation;
use serde_json::Value;
use std::{
    future::Future,
    net::{SocketAddr, TcpListener},
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use test_api::{WorkerSqs, get_sqs_client};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub const WORKER_SQS: WorkerSqs = WorkerSqs::new("product-content-assessment", 60);
const BOUNDARY_TIMEOUT: Duration = Duration::from_secs(100);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub async fn eventually<T>(
    boundary: &str,
    future: impl Future<Output = TestResult<T>>,
) -> TestResult<T> {
    tokio::time::timeout(BOUNDARY_TIMEOUT, future)
        .await
        .map_err(|_| format!("timed out waiting for {boundary}"))?
}

pub async fn case(future: impl Future<Output = TestResult>) {
    // A failed boundary unwinds owned children before the macro tears down queues/DB.
    let result = tokio::time::timeout(Duration::from_secs(240), future).await;
    assert!(
        matches!(&result, Ok(Ok(()))),
        "process acceptance failed: {result:?}"
    );
}

pub fn unused_address() -> TestResult<SocketAddr> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?)
}

pub struct WorkerProcess {
    child: Child,
    reaped: bool,
    coverage_profile: Option<PathBuf>,
}

impl WorkerProcess {
    pub async fn start(
        pool: &sqlx::PgPool,
        relay: &Relay,
        address: SocketAddr,
    ) -> TestResult<Self> {
        let database = pool
            .connect_options()
            .get_database()
            .ok_or("fixture database missing")?
            .to_owned();
        let postgres = url::Url::parse(&test_api::get_postgres_host_gateway_connection_string(
            &database,
        ))?;
        // Keep child profiles in CI's collection directory, but give each child its own
        // file so clean-exit assertions cannot accidentally accept a parent's profile.
        let coverage_profile = std::env::var_os("LLVM_PROFILE_FILE").map(|pattern| {
            PathBuf::from(pattern)
                .with_file_name(format!("worker-{}.profraw", uuid::Uuid::new_v4()))
        });
        // No inherited credentials, endpoint overrides, AWS profiles or paid-provider config.
        // The gateway URL supplies actual fixture credentials/port; this child runs on the host.
        let child = Command::new(env!("CARGO_BIN_EXE_aura-historia-worker"))
            .env_clear()
            .envs(
                coverage_profile
                    .as_ref()
                    .map(|path| ("LLVM_PROFILE_FILE", path)),
            )
            .env("STAGE", "test")
            .env("AWS_REGION", "eu-central-1")
            .env("AWS_ACCESS_KEY_ID", "test")
            .env("AWS_SECRET_ACCESS_KEY", "test")
            .env("AWS_EC2_METADATA_DISABLED", "true")
            .env("AWS_CONFIG_FILE", "/dev/null")
            .env("AWS_SHARED_CREDENTIALS_FILE", "/dev/null")
            .env("AWS_ENDPOINT_URL_SQS", &relay.endpoint)
            .env("AURA_HISTORIA_WORKER_QUEUE_URL", relay.queue_url()?)
            .env("AURA_HISTORIA_WORKER_SCOPE", "product-content-assessment")
            .env("AURA_HISTORIA_WORKER_HEALTH_BIND_ADDR", address.to_string())
            .env("POSTGRES_HOST", "127.0.0.1")
            .env(
                "POSTGRES_PORT",
                postgres
                    .port()
                    .ok_or("fixture Postgres port missing")?
                    .to_string(),
            )
            .env("POSTGRES_DATABASE", database)
            .env("POSTGRES_USERNAME", postgres.username())
            .env(
                "POSTGRES_PASSWORD",
                postgres.password().ok_or("fixture password missing")?,
            )
            .env("POSTGRES_MAX_CONNECTIONS", "2")
            .env("TOKIO_WORKER_THREADS", "2")
            .env("LOG_LEVEL", "warn")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        let mut process = Self {
            child,
            reaped: false,
            coverage_profile,
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .build()?;
        eventually("real worker process readiness", async {
            loop {
                if let Some(status) = process.child.try_wait()? {
                    process.reaped = true;
                    return Err(format!("worker exited before readiness: {status}").into());
                }
                match client.get(format!("http://{address}/ready")).send().await {
                    Ok(response) if response.status().is_success() => return Ok(()),
                    Ok(_) | Err(_) => tokio::time::sleep(POLL_INTERVAL).await,
                }
            }
        })
        .await?;
        Ok(process)
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn assert_running(&mut self) -> TestResult {
        assert!(
            self.child.try_wait()?.is_none(),
            "worker must still own its blocked attempt"
        );
        Ok(())
    }

    pub fn terminate(&mut self) -> TestResult {
        self.assert_running()?;
        let status = Command::new("/usr/bin/kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()?;
        assert!(status.success(), "SIGTERM must reach the owned OS worker");
        Ok(())
    }

    pub async fn wait_for_clean_exit(&mut self) -> TestResult {
        eventually("SIGTERM drain and successful OS exit", async {
            loop {
                if let Some(status) = self.child.try_wait()? {
                    self.reaped = true;
                    assert_eq!(
                        Some(0),
                        status.code(),
                        "worker must drain, not be killed or fail"
                    );
                    if let Some(profile) = &self.coverage_profile {
                        assert!(
                            profile.metadata()?.len() > 0,
                            "clean instrumented worker must flush its own collected profile"
                        );
                    }
                    return Ok(());
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        })
        .await
    }

    pub fn kill(&mut self) -> TestResult {
        if self.reaped || self.child.try_wait()?.is_some() {
            return Err("worker exited before the requested crash boundary".into());
        }
        self.child.kill()?;
        let status = self.child.wait()?;
        self.reaped = true;
        assert_eq!(
            Some(9),
            status.signal(),
            "must kill an actual OS process with SIGKILL"
        );
        Ok(())
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        match self.child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {
                if let Err(error) = self.child.kill() {
                    eprintln!("failed to kill owned worker {}: {error}", self.child.id());
                }
            }
            Err(error) => eprintln!(
                "failed to inspect owned worker {}: {error}",
                self.child.id()
            ),
        }
        if let Err(error) = self.child.wait() {
            eprintln!("failed to reap owned worker {}: {error}", self.child.id());
        }
    }
}

#[derive(Clone, Copy)]
pub struct Source {
    pub product_id: ProductListingId,
    pub event_id: EventId,
}

/// Small discovery seed from product_content_assessment.rs; all assessment logic stays production.
pub async fn commit_source(pool: &sqlx::PgPool) -> TestResult<Source> {
    let source = Source {
        product_id: ProductListingId::new(),
        event_id: EventId::new(),
    };
    let product_id = uuid::Uuid::from(source.product_id);
    let event_id = uuid::Uuid::from(source.event_id);
    let slug = ProductListingSlugId::from_title_and_suffix(
        "content assessment worker product",
        &product_id.simple().to_string()[26..],
    )
    .map_err(|_| "invalid fixture slug")?;
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = uuid::Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query("WITH operator AS (INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, concat($2, '-operator'), 'Fixture operator') RETURNING party_id) INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) SELECT $3, $2, 'Content assessment worker source', party_id FROM operator")
        .bind(party_id).bind(format!("content-assessment-worker-source-{listing_source_id}"))
        .bind(listing_source_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO product_listings (product_listing_id, product_listing_title_slug_id, current_event_id, content_source_event_id, embedding_source_event_id, listing_source_id, source_listing_id, title_text, title_language, description_text, description_language, availability, lifecycle, url, product_images) VALUES ($1, $2, $3, $3, $3, $4, $5, 'Antiker Eichenstuhl', 'de', 'Bemalter Stuhl', 'de', 'AVAILABLE', 'ACTIVE', 'https://example.test/product', '[]')")
        .bind(product_id).bind(slug.as_ref()).bind(event_id)
        .bind(listing_source_id).bind(product_id.to_string()).execute(&mut *tx).await?;
    let payload = serde_json::json!({
        "listingSourceId": listing_source_id.to_string(),
        "sourceListingId": product_id.to_string(),
        "title": {"language": "de", "text": "Antiker Eichenstuhl"},
        "description": {"language": "de", "text": "Bemalter Stuhl"},
        "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
        "availability": "AVAILABLE", "url": "https://example.test/product", "imageCount": 0,
        "auction": {"start": null, "end": null}
    });
    sqlx::query("INSERT INTO product_listing_events (event_id, product_listing_id, event_type, event_group, event_type_schema_version, payload, event_time) VALUES ($1, $2, 'PRODUCT_LISTING_DISCOVERED', 'DOMAIN', 1, $3, now())")
        .bind(event_id).bind(product_id).bind(payload).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(source)
}

fn assert_schema_two_product_event(body: &str, source: Source) -> TestResult {
    let expected = serde_json::json!({
        "schema_version": 2,
        "scope": "product-content-assessment",
        "idempotency_key": format!("product-event:{}", source.event_id),
        "ordering_key": format!("product:{}", source.product_id),
        "job_type": "PRODUCT_LISTING_EVENT",
        "payload": {
            "event_id": source.event_id,
            "product_listing_id": source.product_id,
        }
    });
    assert_eq!(expected, serde_json::from_str::<Value>(body)?);
    Ok(())
}

pub async fn observed_publication(
    observations: &Observations,
    source: Source,
) -> TestResult<(String, String)> {
    let semantic_event_id = source.event_id.to_string();
    let publication = observations
        .wait("successful real schema-2 SQS SendMessage", |events| {
            events.iter().find_map(|event| match event {
                Observation::Sent { body, message_id } if body.contains(&semantic_event_id) => {
                    Some((body.clone(), message_id.clone()))
                }
                _ => None,
            })
        })
        .await?;
    assert_schema_two_product_event(&publication.0, source)?;

    let storage_event_id = uuid::Uuid::from(source.event_id).to_string();
    observations
        .wait(
            "Sequin delivery with raw UUID accepted by child HTTP with 202",
            |events| {
                events
                    .iter()
                    .any(|event| {
                        matches!(event,
                            Observation::Accepted { body }
                                if body.contains(&storage_event_id)
                                    && !body.contains(&semantic_event_id)
                        )
                    })
                    .then_some(())
            },
        )
        .await?;
    Ok(publication)
}

pub async fn observed_publications(
    observations: &Observations,
    source: Source,
    count: usize,
) -> TestResult<Vec<(String, String)>> {
    let semantic_event_id = source.event_id.to_string();
    let publications = observations
        .wait("distinct real schema-2 SQS publications", |events| {
            let sent: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Observation::Sent { body, message_id } if body.contains(&semantic_event_id) => {
                        Some((body.clone(), message_id.clone()))
                    }
                    _ => None,
                })
                .collect();
            (sent.len() >= count).then_some(sent)
        })
        .await?;
    for (body, _) in &publications {
        assert_schema_two_product_event(body, source)?;
    }
    Ok(publications)
}

pub async fn wait_for_blocked_handler(pool: &sqlx::PgPool, blocker_pid: i32) -> TestResult {
    eventually("worker assessment transaction blocked by test DB lock", async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) AND query LIKE '%product_listing_content_assessments%')")
                .bind(blocker_pid).fetch_one(pool).await?;
            if blocked {
                return Ok(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }).await
}

pub async fn wait_for_blocked_handlers(
    pool: &sqlx::PgPool,
    blocker_pid: i32,
    expected: usize,
) -> TestResult<Vec<(i32, Option<String>)>> {
    eventually("both active worker transactions behind the DB write barrier", async {
        loop {
            // B waits on A's product row; A waits on our assessment-table lock.
            let blocked: Vec<(i32, Option<String>)> = sqlx::query_as(
                "WITH RECURSIVE blocked(pid) AS (SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) UNION SELECT a.pid FROM pg_stat_activity a JOIN blocked b ON b.pid = ANY(pg_blocking_pids(a.pid))) SELECT a.pid, a.backend_xid::text FROM pg_stat_activity a JOIN blocked b ON a.pid = b.pid WHERE a.state = 'active' AND a.wait_event_type = 'Lock' AND (a.query LIKE '%product_listings%' OR a.query LIKE '%product_listing_content_assessments%') ORDER BY a.pid",
            ).bind(blocker_pid).fetch_all(pool).await?;
            if blocked.len() == expected {
                return Ok(blocked);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }).await
}

pub async fn wait_for_http_shutdown(address: SocketAddr) -> TestResult {
    eventually("SIGTERM stopped the child HTTP listener", async {
        loop {
            match tokio::net::TcpStream::connect(address).await {
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                    return Ok(());
                }
                // Closing an accepted/pending socket can reset connect before the next
                // probe sees refusal. Neither reset nor EOF alone proves the listener closed.
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                    ) => {}
                Err(error) => return Err(error.into()),
                Ok(socket) => drop(socket),
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
}

pub async fn post_batch(address: SocketAddr, body: &str) -> TestResult {
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()?
        .post(format!("http://{address}/cdc/sequin"))
        .header("content-type", "application/json")
        .timeout(Duration::from_secs(12))
        .body(body.to_owned())
        .send()
        .await?;
    assert_eq!(reqwest::StatusCode::ACCEPTED, response.status());
    Ok(())
}

pub async fn assert_one_source_event(pool: &sqlx::PgPool, source: Source) -> TestResult {
    let events: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT event_id FROM product_listing_events WHERE product_listing_id = $1",
    )
    .bind(uuid::Uuid::from(source.product_id))
    .fetch_all(pool)
    .await?;
    assert_eq!(
        vec![uuid::Uuid::from(source.event_id)],
        events,
        "duplicates must not append another event"
    );
    Ok(())
}

pub async fn assessment(pool: &sqlx::PgPool, source: Source) -> TestResult<Option<Value>> {
    // Include timestamps and tuple identity: duplicate delivery must not even rewrite the row.
    Ok(sqlx::query_scalar("SELECT to_jsonb(assessment) || jsonb_build_object('tuple_version', xmin::text) FROM product_listing_content_assessments assessment WHERE product_listing_id = $1")
        .bind(uuid::Uuid::from(source.product_id)).fetch_optional(pool).await?)
}

pub async fn persisted_assessment(pool: &sqlx::PgPool, source: Source) -> TestResult<Value> {
    let value = eventually("committed content assessment in Postgres", async {
        loop {
            if let Some(value) = assessment(pool, source).await? {
                return Ok(value);
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await?;
    assert_eq!(
        uuid::Uuid::from(source.product_id).to_string(),
        value["product_listing_id"]
    );
    assert_eq!(
        uuid::Uuid::from(source.event_id).to_string(),
        value["source_event_id"]
    );
    assert_eq!("ALLOWED", value["decision"]);
    assert_eq!(Value::Null, value["category"]);
    Ok(value)
}

pub async fn assessment_count(pool: &sqlx::PgPool) -> TestResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM product_listing_content_assessments")
            .fetch_one(pool)
            .await?,
    )
}

pub async fn send(body: &str) -> TestResult<String> {
    let result = get_sqs_client()
        .await
        .send_message()
        .queue_url(WORKER_SQS.queue_url())
        .message_body(body)
        .send()
        .await?;
    Ok(result
        .message_id()
        .ok_or("SQS send missing message ID")?
        .to_owned())
}

/// Operator action only, after the relay confirms the real receive and worker retry settlement.
/// No extra source receives, changed redrive policy, or synthetic receive-count history.
pub async fn make_visible(receipt: &Receipt) -> TestResult {
    get_sqs_client()
        .await
        .change_message_visibility()
        .queue_url(WORKER_SQS.queue_url())
        .receipt_handle(&receipt.handle)
        .visibility_timeout(0)
        .send()
        .await?;
    Ok(())
}

pub async fn assert_native_redrive_policy() -> TestResult {
    let result = get_sqs_client()
        .await
        .get_queue_attributes()
        .queue_url(WORKER_SQS.queue_url())
        .attribute_names(QueueAttributeName::RedrivePolicy)
        .send()
        .await?;
    let policy: Value = serde_json::from_str(
        result
            .attributes()
            .and_then(|attrs| attrs.get(&QueueAttributeName::RedrivePolicy))
            .ok_or("native redrive policy missing")?,
    )?;
    assert_eq!(
        "5",
        policy["maxReceiveCount"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| policy["maxReceiveCount"].to_string())
    );
    assert_eq!(
        "arn:aws:sqs:eu-central-1:000000000000:aura-worker-product-content-assessment-dlq-test",
        policy["deadLetterTargetArn"]
    );
    Ok(())
}

pub async fn dlq_message() -> TestResult<Message> {
    eventually("native SQS DLQ message", async {
        loop {
            let result = get_sqs_client()
                .await
                .receive_message()
                .queue_url(WORKER_SQS.dead_letter_queue_url())
                .max_number_of_messages(1)
                .wait_time_seconds(1)
                .visibility_timeout(0)
                .message_system_attribute_names(MessageSystemAttributeName::All)
                .send()
                .await?;
            if let Some(message) = result.messages().first() {
                return Ok(message.clone());
            }
        }
    })
    .await
}

pub async fn assert_queue_counts(source_messages: usize, dlq_messages: usize) -> TestResult {
    eventually("source/DLQ persisted counts", async {
        loop {
            let mut matches = true;
            for (url, expected) in [
                (WORKER_SQS.queue_url(), source_messages),
                (WORKER_SQS.dead_letter_queue_url(), dlq_messages),
            ] {
                let result = get_sqs_client()
                    .await
                    .get_queue_attributes()
                    .queue_url(url)
                    .attribute_names(QueueAttributeName::ApproximateNumberOfMessages)
                    .attribute_names(QueueAttributeName::ApproximateNumberOfMessagesNotVisible)
                    .attribute_names(QueueAttributeName::ApproximateNumberOfMessagesDelayed)
                    .send()
                    .await?;
                let attrs = result.attributes().ok_or("SQS counts missing")?;
                let mut total = 0;
                for key in [
                    QueueAttributeName::ApproximateNumberOfMessages,
                    QueueAttributeName::ApproximateNumberOfMessagesNotVisible,
                    QueueAttributeName::ApproximateNumberOfMessagesDelayed,
                ] {
                    total += attrs
                        .get(&key)
                        .ok_or("SQS count missing")?
                        .parse::<usize>()?;
                }
                matches &= total == expected;
            }
            if matches {
                return Ok(());
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    })
    .await
}

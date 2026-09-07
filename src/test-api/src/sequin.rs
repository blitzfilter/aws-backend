use crate::{
    IntegrationTestService, get_postgres_client, get_postgres_host_gateway_connection_string,
    postgres::get_postgres_host_port,
};
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::StatusCode;
use sqlx::{AssertSqlSafe, Executor};
use std::future::Future;
use std::net::{SocketAddr, TcpListener};
use std::process::{Command, Stdio};
use std::sync::{Once, OnceLock};
use std::time::Duration;
use testcontainers::core::{Host, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::OnceCell;
use tracing::debug;

const REDIS_CONTAINER_PORT: u16 = 6379;
const REDIS_CONTAINER_NAME_PREFIX: &str = "aura-historia-aws-backend-sequin-redis-test";
const SEQUIN_CONTAINER_PORT: u16 = 7376;
const SEQUIN_CONTAINER_NAME_PREFIX: &str = "aura-historia-aws-backend-sequin-test";
const SEQUIN_READINESS_TIMEOUT: Duration = Duration::from_secs(90);
const SEQUIN_READINESS_POLL_INTERVAL: Duration = Duration::from_millis(200);
const SEQUIN_START_ATTEMPTS: usize = 3;
const SEQUIN_START_RETRY_DELAY: Duration = Duration::from_secs(1);
const SEQUIN_IMAGE_TAG: &str = "v0.14.6";
const SEQUIN_STATE_DB_PREFIX: &str = "sequin";
const SECRET_KEY_BASE: &str = "wDPLYus0pvD6qJhKJICO4vYl782Zjtpew5qRBDp7CZvbWtQmY0eB13If01234567";
const VAULT_KEY: &str = "2Sig69bIpuSm2kv0VQfDekET2qy8qUZGI8v3/h3ASiY=";
const WORKER_WEBHOOK_TABLES: &[&str] = &[
    "public.product_listing_events",
    "public.search_filters",
    "public.search_filter_matches",
];
const NOTIFICATION_DELIVERY_TABLE: &str = "public.notification_deliveries";
const PRODUCT_LISTING_RAW_REVISIONS_TABLE: &str = "public.product_listing_raw_revisions";

static WORKER_WEBHOOK_SEQUIN: OnceCell<RunningSequin> = OnceCell::const_new();
static WORKER_WEBHOOK_PORT: OnceLock<u16> = OnceLock::new();

fn redis_container_name() -> String {
    format!("{REDIS_CONTAINER_NAME_PREFIX}-{}", std::process::id())
}

fn sequin_container_name() -> String {
    format!("{SEQUIN_CONTAINER_NAME_PREFIX}-{}", std::process::id())
}

fn docker_remove(name: &str) -> std::io::Result<std::process::ExitStatus> {
    Command::new("docker")
        .args(["rm", "-f", name])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
}

extern "C" fn cleanup() {
    let _ = docker_remove(&sequin_container_name());
    let _ = docker_remove(&redis_container_name());
}

/// Installs cleanup hooks for the process-lived Redis and Sequin containers.
///
/// The hooks remove both containers on normal exit and on SIGINT or SIGTERM.
fn install_cleanup() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        unsafe { libc::atexit(cleanup) };
        crate::signal::register_signal_cleanup(|| cleanup());
    });
}

/// Process-lived Sequin fixture that delivers the worker's CDC tables to its local webhook.
#[derive(Debug, Clone, Copy)]
pub struct Sequin;

impl Sequin {
    pub const fn worker_webhook() -> Self {
        Self
    }
}

#[async_trait]
impl IntegrationTestService for Sequin {
    fn service_names(&self) -> &'static [&'static str] {
        &[]
    }

    async fn set_up(&self) {
        get_or_start_worker_webhook_sequin().await;
    }
}

#[derive(Debug)]
struct RunningSequin {
    _redis: ContainerAsync<GenericImage>,
    _sequin: ContainerAsync<GenericImage>,
}

/// Returns the fixed process-local address that the worker must bind before source writes.
pub fn get_sequin_worker_webhook_bind_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], worker_webhook_port()))
}

async fn get_or_start_worker_webhook_sequin() -> &'static RunningSequin {
    WORKER_WEBHOOK_SEQUIN
        .get_or_init(|| async {
            let webhook_url = format!(
                "http://host.docker.internal:{}/cdc/sequin",
                worker_webhook_port()
            );

            retry_sequin_start(SEQUIN_START_RETRY_DELAY, || {
                start_worker_webhook_sequin(&webhook_url)
            })
            .await
            .unwrap_or_else(|error| panic!("Sequin test fixture did not start: {error}"))
        })
        .await
}

async fn retry_sequin_start<T, F, Fut>(retry_delay: Duration, mut start: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, String>>,
{
    for attempt in 1..=SEQUIN_START_ATTEMPTS {
        match start().await {
            Ok(result) => return Ok(result),
            Err(error) if attempt < SEQUIN_START_ATTEMPTS => {
                debug!(attempt, %error, "Sequin test fixture startup failed; retrying.");
                tokio::time::sleep(retry_delay).await;
            }
            Err(error) => {
                return Err(format!(
                    "Sequin test fixture did not start after {SEQUIN_START_ATTEMPTS} attempts: {error}"
                ));
            }
        }
    }

    unreachable!("Sequin startup loop always returns or fails")
}

async fn start_worker_webhook_sequin(webhook_url: &str) -> Result<RunningSequin, String> {
    install_cleanup();

    let suffix = std::process::id().to_string();
    let state_database = format!("{SEQUIN_STATE_DB_PREFIX}_{suffix}");
    ensure_sequin_state_database(&state_database).await?;

    let redis_name = redis_container_name();
    let sequin_name = sequin_container_name();
    // A reused PID can leave containers from an aborted earlier test process.
    let _ = docker_remove(&redis_name);
    let _ = docker_remove(&sequin_name);

    let redis_port = find_free_port();
    let redis_started = std::time::Instant::now();
    let redis = GenericImage::new("redis", "7.4.2-alpine")
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .with_container_name(redis_name)
        .with_mapped_port(redis_port, REDIS_CONTAINER_PORT.tcp())
        .start()
        .await
        .map_err(|error| format!("failed starting Redis test container for Sequin: {error}"))?;

    debug!(
        elapsed_ms = redis_started.elapsed().as_millis(),
        "Sequin Redis container ready."
    );

    let config_yaml = sequin_config_yaml(webhook_url, &suffix);
    let config_yaml_base64 = STANDARD.encode(config_yaml);
    let redis_url = format!("redis://host.docker.internal:{redis_port}");
    let sequin_state_pg_url = get_postgres_host_gateway_connection_string(&state_database);

    let sequin_port = find_free_port();
    let sequin_started = std::time::Instant::now();
    let sequin = GenericImage::new("sequin/sequin", SEQUIN_IMAGE_TAG)
        .with_env_var("SERVER_PORT", SEQUIN_CONTAINER_PORT.to_string())
        .with_env_var("PG_URL", sequin_state_pg_url)
        .with_env_var("PG_POOL_SIZE", "3")
        .with_env_var("REDIS_URL", redis_url)
        .with_env_var("SECRET_KEY_BASE", SECRET_KEY_BASE)
        .with_env_var("VAULT_KEY", VAULT_KEY)
        .with_env_var("CONFIG_FILE_YAML", config_yaml_base64)
        .with_env_var("TELEMETRY_ENABLED", "false")
        .with_env_var("CRASH_REPORTING_DISABLED", "true")
        .with_host("host.docker.internal", Host::HostGateway)
        .with_container_name(sequin_name)
        .with_mapped_port(sequin_port, SEQUIN_CONTAINER_PORT.tcp())
        .start()
        .await
        .map_err(|error| format!("failed starting Sequin test container: {error}"))?;
    let endpoint_url = format!("http://localhost:{sequin_port}");

    debug!(
        elapsed_ms = sequin_started.elapsed().as_millis(),
        "Sequin container process started."
    );
    wait_for_sequin_health(&endpoint_url, &sequin).await?;
    wait_for_worker_webhook_replication(&format!("aura_historia_test_slot_{suffix}"), &sequin)
        .await?;
    debug!(%endpoint_url, "Successfully started process-lived Sequin test container.");

    Ok(RunningSequin {
        _redis: redis,
        _sequin: sequin,
    })
}

async fn ensure_sequin_state_database(database: &str) -> Result<(), String> {
    let pool = get_postgres_client().await;
    let exists: bool = sqlx::query_scalar(AssertSqlSafe(
        "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1)",
    ))
    .bind(database)
    .fetch_one(&pool)
    .await
    .map_err(|error| format!("failed checking Sequin state database {database}: {error}"))?;

    if !exists {
        pool.execute(AssertSqlSafe(format!("CREATE DATABASE {database}")))
            .await
            .map_err(|error| {
                format!("failed creating Sequin state database {database}: {error}")
            })?;
    }
    Ok(())
}

fn find_free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("shouldn't fail binding to a random port")
        .local_addr()
        .expect("shouldn't fail reading local address")
        .port()
}

fn worker_webhook_port() -> u16 {
    *WORKER_WEBHOOK_PORT.get_or_init(find_free_port)
}

fn sequin_config_yaml(webhook_url: &str, suffix: &str) -> String {
    let publication_tables = WORKER_WEBHOOK_TABLES
        .iter()
        .copied()
        .chain(std::iter::once(NOTIFICATION_DELIVERY_TABLE))
        .chain(std::iter::once(PRODUCT_LISTING_RAW_REVISIONS_TABLE))
        .collect::<Vec<_>>()
        .join(", ");
    let include_tables = WORKER_WEBHOOK_TABLES
        .iter()
        .map(|table| format!("\"{table}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut config = include_str!("sequin/base.yaml")
        .replace("__SUFFIX__", suffix)
        .replace("__POSTGRES_PORT__", &get_postgres_host_port().to_string())
        .replace("__PUBLICATION_TABLES__", &publication_tables);

    let sink_yaml = include_str!("sequin/webhook-sink.yaml")
        .replace("__SUFFIX__", suffix)
        .replace("__WEBHOOK_URL__", webhook_url)
        .replace("__INCLUDE_TABLES__", &include_tables);
    config.push_str(&sink_yaml);

    let notification_delivery_sink_yaml =
        include_str!("sequin/notification-delivery-webhook-sink.yaml")
            .replace("__SUFFIX__", suffix)
            .replace("__WEBHOOK_URL__", webhook_url);
    config.push_str(&format!(
        "  {}\n",
        notification_delivery_sink_yaml
            .trim_end()
            .replace('\n', "\n  ")
    ));

    let product_listing_normalization_sink_yaml =
        include_str!("sequin/product-listing-normalization-webhook-sink.yaml")
            .replace("__SUFFIX__", suffix)
            .replace("__WEBHOOK_URL__", webhook_url);
    config.push_str(&format!(
        "  {}\n",
        product_listing_normalization_sink_yaml
            .trim_end()
            .replace('\n', "\n  ")
    ));

    config
}

async fn wait_for_sequin_health(
    endpoint_url: &str,
    container: &ContainerAsync<GenericImage>,
) -> Result<(), String> {
    let client = reqwest::Client::new();
    let health_url = format!("{endpoint_url}/health");

    let started = std::time::Instant::now();
    while started.elapsed() < SEQUIN_READINESS_TIMEOUT {
        if let Ok(response) = client.get(&health_url).send().await
            && response.status() == StatusCode::OK
        {
            debug!(
                elapsed_ms = started.elapsed().as_millis(),
                "Sequin health endpoint ready."
            );
            return Ok(());
        }
        tokio::time::sleep(SEQUIN_READINESS_POLL_INTERVAL).await;
    }

    Err(sequin_failure_message(
        container,
        format!("Sequin health endpoint did not become ready at {health_url}"),
    )
    .await)
}

async fn wait_for_worker_webhook_replication(
    slot_name: &str,
    container: &ContainerAsync<GenericImage>,
) -> Result<(), String> {
    let pool = get_postgres_client().await;

    let started = std::time::Instant::now();
    while started.elapsed() < SEQUIN_READINESS_TIMEOUT {
        let active = sqlx::query_scalar(AssertSqlSafe(
            "SELECT EXISTS(SELECT 1 FROM pg_replication_slots WHERE slot_name = $1 AND active)",
        ))
        .bind(slot_name)
        .fetch_one(&pool)
        .await;

        match active {
            Ok(true) => {
                debug!(
                    elapsed_ms = started.elapsed().as_millis(),
                    "Sequin replication slot active."
                );
                return Ok(());
            }
            Ok(false) => {}
            Err(error) => debug!(%slot_name, %error, "Sequin replication slot is not ready yet."),
        }
        tokio::time::sleep(SEQUIN_READINESS_POLL_INTERVAL).await;
    }

    Err(sequin_failure_message(
        container,
        format!("Sequin replication slot did not become active: {slot_name}"),
    )
    .await)
}

async fn sequin_failure_message(
    container: &ContainerAsync<GenericImage>,
    message: String,
) -> String {
    let stdout = container.stdout_to_vec().await.unwrap_or_default();
    let stderr = container.stderr_to_vec().await.unwrap_or_default();
    format_sequin_failure_message(message, &stdout, &stderr)
}

fn format_sequin_failure_message(message: String, stdout: &[u8], stderr: &[u8]) -> String {
    format!(
        "{message}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
}

#[cfg(test)]
mod tests {
    use super::{SEQUIN_START_ATTEMPTS, format_sequin_failure_message, retry_sequin_start};
    use std::time::Duration;

    #[tokio::test]
    async fn should_retry_sequin_start_after_transient_failure() {
        let mut attempts = 0;

        let result = retry_sequin_start(Duration::ZERO, || {
            attempts += 1;
            std::future::ready(if attempts == 1 {
                Err("transient failure".to_owned())
            } else {
                Ok("started")
            })
        })
        .await;

        assert_eq!(result, Ok("started"));
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn should_return_last_sequin_start_failure_after_all_attempts() {
        let mut attempts = 0;

        let result = retry_sequin_start(Duration::ZERO, || {
            attempts += 1;
            std::future::ready(Err::<(), _>("transient failure".to_owned()))
        })
        .await;

        assert_eq!(
            result,
            Err(format!(
                "Sequin test fixture did not start after {SEQUIN_START_ATTEMPTS} attempts: transient failure"
            ))
        );
        assert_eq!(attempts, SEQUIN_START_ATTEMPTS);
    }

    #[test]
    fn should_include_sequin_logs_in_startup_failure_message() {
        assert_eq!(
            format_sequin_failure_message(
                "startup failed".to_owned(),
                b"container stdout",
                b"container stderr"
            ),
            "startup failed\nstdout:\ncontainer stdout\nstderr:\ncontainer stderr"
        );
    }
}

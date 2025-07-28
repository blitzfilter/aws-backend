use aws_config::{BehaviorVersion, SdkConfig};
use aws_sdk_dynamodb::config::Credentials;
use std::collections::HashMap;
use testcontainers::core::{IntoContainerPort, Mount};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::localstack::LocalStack;
use tokio::sync::OnceCell;
use tracing::{debug, error};

/// A lazily-initialized, globally accessible AWS SDK configuration for integration tests.
///
/// This static `OnceCell` holds the result of `aws_config::load()` with LocalStack-specific
/// overrides (e.g., test credentials, custom endpoint, region).
///
/// Initialized once on first use via [`get_aws_config()`].
static CONFIG: OnceCell<SdkConfig> = OnceCell::const_new();

/// Loads and returns a static reference to the AWS SDK configuration for LocalStack.
///
/// This function ensures that the configuration is loaded only once using `OnceCell`.
/// It configures the AWS SDK to use:
/// - Test credentials (`Credentials::for_tests()`)
/// - Static region (`"eu-central-1"`)
/// - LocalStack endpoint at [Endpoint-URL](get_endpoint_url)
///
/// # Returns
///
/// A reference to a globally-initialized `SdkConfig` instance suitable for use with AWS clients.
pub async fn get_aws_config() -> &'static SdkConfig {
    let cfg = CONFIG
        .get_or_init(|| async {
            aws_config::defaults(BehaviorVersion::latest())
                .credentials_provider(Credentials::for_tests())
                .region("eu-central-1")
                .endpoint_url("http://localhost:4566")
                .load()
                .await
        })
        .await;
    debug!("Successfully set up AWS-Config.");
    cfg
}

/// Spins up a LocalStack container with custom environment variables.
///
/// This function uses [`testcontainers`] to start a LocalStack Docker container with:
/// - Optional environment variables (e.g., AWS services to enable)
/// - Mounted Docker socket (for container-in-container support)
/// - Port 4566 mapped for API access
///
/// It also sets up structured JSON tracing using `tracing_subscriber`.
///
/// # Arguments
///
/// * `env_vars` - A map of environment variables to pass to the LocalStack container.
///
/// # Returns
///
/// A running [`ContainerAsync<LocalStack>`] instance, ready for AWS SDK interactions.
///
/// # Panics
///
/// Panics if the container fails to start.
pub async fn spin_up_localstack(env_vars: HashMap<&str, &str>) -> ContainerAsync<LocalStack> {
    let _ = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_current_span(true)
        .with_ansi(false)
        .try_init();
    debug!("Successfully initialized tracing_subscriber.");

    let request = env_vars
        .iter()
        .fold(LocalStack::default().with_tag("latest"), |ls, (k, v)| {
            ls.with_env_var(*k, *v)
        })
        .with_mount(Mount::bind_mount(
            "/var/run/docker.sock",
            "/var/run/docker.sock",
        ))
        .with_mapped_port(4566, 4566.tcp());

    let container = request
        .start()
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to start LocalStack.");
            e
        })
        .unwrap();
    debug!("Successfully started LocalStack-Container.");
    container
}

/// Spins up a LocalStack container with the specified AWS services enabled.
///
/// This is a convenience wrapper over [`spin_up_localstack`], which builds the `SERVICES`
/// environment variable string from the provided list.
///
/// # Arguments
///
/// * `services` - A list of AWS service identifiers (e.g., `"s3"`, `"dynamodb"`).
///
/// # Returns
///
/// A running [`ContainerAsync<LocalStack>`] with only the requested services enabled.
pub async fn spin_up_localstack_with_services(services: &[&str]) -> ContainerAsync<LocalStack> {
    spin_up_localstack(HashMap::from([("SERVICES", services.join(",").as_str())])).await
}

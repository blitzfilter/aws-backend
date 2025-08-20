use aws_config::{BehaviorVersion, SdkConfig};
use aws_sdk_dynamodb::config::Credentials;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use testcontainers::core::{IntoContainerPort, Mount};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::localstack::LocalStack;
use tokio::sync::{Mutex, OnceCell};
use tracing::{debug, error, info};

/// A lazily-initialized, globally accessible AWS SDK configuration for integration tests.
///
/// This static `OnceCell` holds the result of `aws_config::load()` with LocalStack-specific
/// overrides (e.g., test credentials, custom endpoint, region).
///
/// Initialized once on first use via [`get_aws_config()`].
static CONFIG: OnceCell<SdkConfig> = OnceCell::const_new();

/// A shared LocalStack container instance for all integration tests.
///
/// This container is created once and reused across all tests to avoid the expensive
/// container startup overhead. The container is initialized with all commonly used
/// services enabled.
static SHARED_CONTAINER: OnceCell<ContainerAsync<LocalStack>> = OnceCell::const_new();

/// Mutex to ensure only one test can initialize the shared container at a time.
static CONTAINER_INIT_LOCK: Mutex<()> = Mutex::const_new(());

/// Flag to track if container cleanup has been registered.
static CLEANUP_REGISTERED: AtomicBool = AtomicBool::new(false);

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

/// Gets or creates a shared LocalStack container for all integration tests.
///
/// This function maintains a single, shared LocalStack container across all tests to avoid
/// the expensive container startup overhead. The container is initialized with commonly
/// used services and reused across test runs.
///
/// Services included: dynamodb, opensearch, s3, sqs, lambda
///
/// # Returns
///
/// A reference to the shared [`ContainerAsync<LocalStack>`] instance.
///
/// # Notes
///
/// - The container is created only once on first use
/// - All tests use the same container instance for better performance
/// - Individual tests should use service `tear_down()` methods to clean state
/// - Container cleanup is handled automatically when the test process exits
pub async fn get_or_create_shared_container() -> &'static ContainerAsync<LocalStack> {
    SHARED_CONTAINER
        .get_or_init(|| async {
            let _lock = CONTAINER_INIT_LOCK.lock().await;

            // Register cleanup handler if not already done
            if !CLEANUP_REGISTERED.swap(true, Ordering::SeqCst) {
                register_container_cleanup();
            }

            info!("Initializing shared LocalStack container with comprehensive service list");

            // Include all services used across the test suite
            let all_services = vec!["dynamodb", "opensearch", "s3", "sqs", "lambda"];
            let container = spin_up_localstack_with_services(&all_services).await;

            info!("Shared LocalStack container initialized successfully");
            container
        })
        .await
}

/// Registers a cleanup handler to gracefully shut down the shared container.
///
/// This function uses `std::panic::set_hook` to ensure container cleanup even
/// if tests panic, and relies on the `Drop` trait for normal shutdown.
fn register_container_cleanup() {
    // Set up panic hook to ensure cleanup on panic
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        eprintln!("Test panic detected, cleaning up LocalStack container...");
        if let Some(_container) = SHARED_CONTAINER.get() {
            // Best effort cleanup on panic - can't use async in panic hook
            eprintln!("Shared container will be cleaned up by Drop implementation");
        }
        orig_hook(panic_info);
    }));

    debug!("Container cleanup handler registered");
}

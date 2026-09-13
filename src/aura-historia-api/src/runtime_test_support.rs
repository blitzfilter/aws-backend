use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
    time::{Duration, Instant},
};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) fn inputs() -> BTreeMap<&'static str, String> {
    [
        ("STAGE", "ephemeral"),
        ("COMMIT_SHA", "unversioned"),
        ("POSTGRES_SSL_MODE", "disable"),
        ("POSTGRES_HOST", "127.0.0.1"),
        ("POSTGRES_DATABASE", "postgres"),
        ("POSTGRES_USERNAME", "postgres"),
        ("POSTGRES_PASSWORD", "password_canary"),
        ("AURA_HISTORIA_API_BIND_ADDR", "127.0.0.1:0"),
        ("AURA_HISTORIA_API_OPERATIONS_BIND_ADDR", "127.0.0.1:0"),
        (
            "AURA_HISTORIA_COGNITO_ISSUER",
            "https://cognito-idp.eu-west-1.amazonaws.com/test-pool",
        ),
        ("AURA_HISTORIA_COGNITO_JWKS_URL", "http://127.0.0.1:1/jwks"),
        ("AURA_HISTORIA_COGNITO_APP_CLIENT_IDS", "test-client"),
        ("AURA_HISTORIA_COGNITO_USER_POOL_ID", "test-pool"),
        ("STRIPE_API_KEY", "stripe_secret_canary"),
        (
            "STRIPE_CHECKOUT_SUCCESS_URL",
            "https://example.test/success",
        ),
        ("STRIPE_CHECKOUT_CANCEL_URL", "https://example.test/cancel"),
        ("STRIPE_PORTAL_RETURN_URL", "https://example.test/return"),
        ("STRIPE_PRO_MONTHLY_PRICE_ID", "price_test_pm"),
        ("STRIPE_PRO_YEARLY_PRICE_ID", "price_test_py"),
        ("STRIPE_ULTIMATE_MONTHLY_PRICE_ID", "price_test_um"),
        ("STRIPE_ULTIMATE_YEARLY_PRICE_ID", "price_test_uy"),
        ("ZOHO_LIST_KEY", "list_test"),
        ("ZOHO_CLIENT_ID", "client_test"),
        ("ZOHO_CLIENT_SECRET", "zoho_secret_canary"),
        ("ZOHO_REFRESH_TOKEN", "refresh_secret_canary"),
        ("ZOHO_ACCOUNTS_URL", "http://127.0.0.1:1"),
        ("ZOHO_CAMPAIGNS_URL", "http://127.0.0.1:1"),
        ("OPENSEARCH_ENDPOINT_URL", "http://127.0.0.1:1"),
        ("VERTEX_AI_PROJECT_ID", "not-a-cloud-project"),
        ("VERTEX_AI_LOCATION", "eu"),
        ("AWS_EC2_METADATA_DISABLED", "true"),
        ("AWS_REGION", "eu-west-1"),
    ]
    .map(|(key, value)| (key, value.to_owned()))
    .into()
}

pub(super) struct OwnedDirectory(pub(super) PathBuf);

impl OwnedDirectory {
    pub(super) fn new() -> TestResult<Self> {
        let path =
            std::env::temp_dir().join(format!("aura-api-lifecycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }

    pub(super) fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("owned test-directory cleanup failed: {}", error.kind());
        }
    }
}

pub(super) struct OwnedChild(pub(super) Child);

impl OwnedChild {
    pub(super) fn wait(&mut self, budget: Duration) -> TestResult<ExitStatus> {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err("child exceeded test deadline".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(super) fn signal(&self, signal: &str) -> TestResult {
        let status = Command::new("kill")
            .arg(signal)
            .arg(self.0.id().to_string())
            .status()?;
        if !status.success() {
            return Err("failed to signal owned child".into());
        }
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        match self.0.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                if let Err(error) = self.0.kill() {
                    eprintln!("owned child kill failed: {}", error.kind());
                }
                if let Err(error) = self.0.wait() {
                    eprintln!("owned child reap failed: {}", error.kind());
                }
            }
        }
    }
}

pub(super) async fn wait_for_file(path: &Path) -> TestResult<String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match std::fs::read_to_string(path) {
                Ok(value) if !value.is_empty() => return Ok(value),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
    .map_err(Into::into)
}

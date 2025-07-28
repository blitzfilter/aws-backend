use crate::IntegrationTestService;
use crate::localstack::get_aws_config;
use async_trait::async_trait;
use aws_sdk_lambda::Client;
use aws_sdk_lambda::client::Waiters;
use aws_sdk_lambda::types::Runtime::Providedal2023;
use derive_builder::Builder;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::{env, fs};
use tokio::sync::OnceCell;
use tracing::debug;
use walkdir::WalkDir;

/// A lazily-initialized, globally shared Lambda client for integration testing.
///
/// This `OnceCell` ensures that the client is only created once during the test lifecycle,
/// using the shared [`SdkConfig`] provided by [`get_aws_config()`].
static LAMBDA_CLIENT: OnceCell<Client> = OnceCell::const_new();

/// Returns a shared `aws_sdk_lambda::Client` for interacting with LocalStack.
///
/// The client is initialized only once using a global `OnceCell`, and internally depends on
/// [`get_aws_config()`] for configuration (test credentials, region, LocalStack endpoint).
///
/// # Returns
///
/// A reference to a lazily-initialized `Client` instance.
pub async fn get_lambda_client() -> &'static Client {
    let client = LAMBDA_CLIENT
        .get_or_init(|| async { Client::new(get_aws_config().await) })
        .await;
    debug!("Successfully initialized Lambda-Client.");
    client
}

/// Marker type representing the Lambda service in LocalStack-based tests.
///
/// Implements the [`IntegrationTestService`] trait to support lifecycle management
/// when used with the `#[localstack_test]` macro.
#[derive(Debug, Builder)]
pub struct Lambda {
    pub name: &'static str,
    pub path: &'static str,
    #[builder(setter(strip_option), default)]
    pub role: Option<&'static str>,
}

#[async_trait]
impl IntegrationTestService for Lambda {
    fn service_names(&self) -> &'static [&'static str] {
        &["lambda"]
    }

    async fn set_up(&self) {
        let lambda_zip_path = build_lambda_if_needed(self.name, Path::new(self.path));
        let mut file = File::open(lambda_zip_path)
            .unwrap_or_else(|err| panic!("shouldn't fail opening file {}: {}", self.path, err));
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .unwrap_or_else(|err| panic!("shouldn't fail reading file {}: {}", self.path, err));

        let client = get_lambda_client().await;

        client
            .create_function()
            .function_name(self.name)
            .runtime(Providedal2023)
            .handler("lib.handler")
            .role(
                self.role
                    .unwrap_or("arn:aws:iam::000000000000:role/service-role/dummy"),
            )
            .code(
                aws_sdk_lambda::types::FunctionCode::builder()
                    .zip_file(buffer.into())
                    .build(),
            )
            .send()
            .await
            .unwrap_or_else(|err| {
                panic!("shouldn't fail creating lambda with config {self:?} but did with: {err}")
            });
        debug!(lambdaName = self.name, "Successfully created lambda.");

        client
            .wait_until_function_active_v2()
            .function_name(self.name)
            .wait(Duration::from_secs(30))
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "shouldn't fail waiting for lambda {} to become active, but did with: {err}",
                    self.name
                )
            });
        debug!(lambdaName = self.name, "Successfully activated lambda.");
    }
}

/// Builds a named lambda if not cached.
/// Returns the path to the `.zip` file.
#[tracing::instrument(
    skip(lambda_name, lambda_src_dir),
    fields(
        lambdaName = lambda_name,
        lambdaSrcDir = lambda_src_dir
            .to_str()
            .expect("shouldn't fail unwrapping via Path::to_str")
    )
)]
pub fn build_lambda_if_needed(lambda_name: &str, lambda_src_dir: &Path) -> PathBuf {
    let hash = hash_dir(lambda_src_dir);

    let cache_dir = PathBuf::from("/tmp/lambdas");
    let output_zip = cache_dir.join(format!("{lambda_name}_{hash}.zip"));

    if output_zip.exists() {
        debug!("Lambda already exists in cache. Skipping rebuild.");
        return output_zip;
    }

    debug!("Building lambda...");

    fs::create_dir_all(&cache_dir).unwrap();

    let status = Command::new("cargo")
        .arg("lambda")
        .arg("build")
        .arg("--release")
        .arg("--output-format")
        .arg("zip")
        .arg("--manifest-path")
        .arg("Cargo.toml")
        .arg("--target-dir")
        .arg("target")
        .arg("--package")
        .arg(lambda_name)
        .arg("--bin")
        .arg(lambda_name)
        .current_dir(lambda_src_dir)
        .status()
        .unwrap_or_else(|err| panic!("shouldn't fail building lambda '{lambda_name}': {err}"));
    debug!(status = %status, "Finished building lambda");
    assert!(status.success(), "Lambda build failed");

    // Clean up old cached zips for this lambda
    if let Ok(entries) = fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(fname) = path.file_name().and_then(|f| f.to_str()) {
                if fname.starts_with(&format!("{lambda_name}_")) && fname.ends_with(".zip") {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }

    // thanks to: https://github.com/rust-lang/cargo/issues/3946#issuecomment-973132993
    let workspace_root = env::var("CARGO_WORKSPACE_DIR")
        .expect("shouldn't fail because environment-variable 'CARGO_WORKSPACE_DIR' is set.");
    let built_zip = Path::new(&workspace_root)
        .join("target/lambda")
        .join(lambda_name)
        .join("bootstrap.zip");

    fs::copy(&built_zip, &output_zip).unwrap_or_else(|err| {
        panic!("shouldn't fail copying zip for lambda '{lambda_name}': {err}")
    });

    output_zip
}

/// Computes an SHA-256 hash of all files in a directory.
fn hash_dir(dir: &Path) -> String {
    let mut hasher = Sha256::new();

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let contents = fs::read(path).unwrap_or_default();
        hasher.update(contents);
    }

    let result = hasher.finalize();
    hex::encode(result)
}

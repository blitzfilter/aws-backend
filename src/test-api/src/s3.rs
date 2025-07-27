use crate::IntegrationTestService;
use async_trait::async_trait;

/// Marker type representing the S3 service in LocalStack-based tests.
///
/// **This currently is a no-op and only starts the service for LocalStack - includes no custom set-up logic.**
///
/// Implements the [`IntegrationTestService`] trait to support lifecycle management
/// when used with the `#[localstack_test]` macro.
pub struct S3();

#[async_trait]
impl IntegrationTestService for S3 {
    const SERVICE_NAMES: &'static [&'static str] = &["s3"];

    async fn set_up() {}
}

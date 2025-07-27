use test_api::*;

#[localstack_test(services = [S3()])]
async fn should_run_without_errors() {}

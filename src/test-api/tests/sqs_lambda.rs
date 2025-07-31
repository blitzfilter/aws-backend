use test_api::*;

const SQS: Sqs = Sqs { name: "test_sqs" };
const LAMBDA: Lambda = Lambda {
    name: "test_lambda",
    path: "src/test_lambda",
    role: None,
};

const SQS_LAMBDA: SqsLambdaEventSourceMapping = SqsLambdaEventSourceMapping {
    sqs: &SQS,
    lambda: &LAMBDA,
    max_batch_size: 1,
    max_batch_window_seconds: 0,
};

#[localstack_test(services = [SQS_LAMBDA])]
async fn should_run_without_errors() {}

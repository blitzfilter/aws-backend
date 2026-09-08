import * as cdk from "aws-cdk-lib";
import * as fs from "node:fs";
import * as path from "node:path";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationDataStack, ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { stageConfig, STAGES, type StageName } from "../src/config";
import { QUEUE_DEFINITIONS } from "../src/constructs/queues";
import { importWorkerQueueCatalog, WorkerQueues } from "../src/constructs/worker-queues";
import { WORKER_QUEUE_DEFINITIONS, WORKER_SCOPES, workerQueueName, type WorkerScope } from "../src/worker-queue-config";

// Independent contract: changing the catalog must not silently change the runtime boundary.
const EXPECTED_WORKERS = {
  "product-listing-opensearch": { id: "ProductListingOpensearch", visibility: 60 },
  "search-filter-projection": { id: "SearchFilterProjection", visibility: 60 },
  "search-filter-percolator": { id: "SearchFilterPercolator", visibility: 300 },
  "search-filter-match-notification": { id: "SearchFilterMatchNotification", visibility: 60 },
  "watchlist-notification": { id: "WatchlistNotification", visibility: 60 },
  "product-content-assessment": { id: "ProductContentAssessment", visibility: 60 },
  "product-embedding": { id: "ProductEmbedding", visibility: 300 },
  "product-translation": { id: "ProductTranslation", visibility: 300 },
  "product-listing-normalization": { id: "ProductListingNormalization", visibility: 300 },
  "notification-delivery": { id: "NotificationDelivery", visibility: 360 },
} as const;
const EXPECTED_SCOPES = Object.keys(EXPECTED_WORKERS) as WorkerScope[];

function queueResource(template: Template, name: string) {
  const matches = Object.entries(template.findResources("AWS::SQS::Queue"))
    .filter(([, resource]) => resource.Properties.QueueName === name);
  expect(matches).toHaveLength(1);
  const [id, resource] = matches[0];
  return { id, resource, arn: { "Fn::GetAtt": [id, "Arn"] }, url: { Ref: id } };
}

function expectWorkerPair(stack: cdk.Stack, template: Template, stage: StageName, workerScope: WorkerScope) {
  const definition = EXPECTED_WORKERS[workerScope];
  const sourceName = `aura-worker-${workerScope}-${stage}`;
  const dlqName = `aura-worker-${workerScope}-dlq-${stage}`;
  const source = queueResource(template, sourceName);
  const dlq = queueResource(template, dlqName);
  const lifecycle = stage === "prod" ? "Retain" : "Delete";
  const sharedProperties = { ReceiveMessageWaitTimeSeconds: 20, SqsManagedSseEnabled: true };
  expect(source.resource).toEqual({
    Type: "AWS::SQS::Queue",
    Properties: {
      ...sharedProperties,
      QueueName: sourceName,
      MessageRetentionPeriod: 604800,
      VisibilityTimeout: definition.visibility,
      RedrivePolicy: { deadLetterTargetArn: dlq.arn, maxReceiveCount: 5 },
      RedriveAllowPolicy: { redrivePermission: "denyAll" },
    },
    DeletionPolicy: lifecycle,
    UpdateReplacePolicy: lifecycle,
  });
  expect(dlq.resource).toEqual({
    Type: "AWS::SQS::Queue",
    Properties: {
      ...sharedProperties,
      QueueName: dlqName,
      MessageRetentionPeriod: 1209600,
      RedriveAllowPolicy: {
        redrivePermission: "byQueue",
        sourceQueueArns: [stack.resolve(stack.formatArn({ service: "sqs", resource: sourceName }))],
      },
    },
    DeletionPolicy: lifecycle,
    UpdateReplacePolicy: lifecycle,
  });

  for (const queue of [source, dlq]) {
    // CDK omits FifoQueue=false; absent is Standard in CloudFormation and GetQueueAttributes.
    expect(queue.resource.Properties.FifoQueue).toBeUndefined();
    expect(queue.resource.Properties.QueueName).not.toMatch(/\.fifo$/);
    expect(queue.resource.Properties.QueueName.length).toBeLessThanOrEqual(80);
    const policies = Object.values(template.findResources("AWS::SQS::QueuePolicy"))
      .filter((policy) => JSON.stringify(policy.Properties.Queues) === JSON.stringify([queue.url]));
    expect(policies).toHaveLength(1);
    // No public/service Allow, wildcard resource, or extra condition that could bypass TLS.
    expect(policies[0].Properties.PolicyDocument).toEqual({
      Version: "2012-10-17",
      Statement: [{
        Action: "sqs:*",
        Effect: "Deny",
        Principal: { AWS: "*" },
        Resource: queue.arn,
        Condition: { Bool: { "aws:SecureTransport": "false" } },
      }],
    });
  }

  const outputs = template.toJSON().Outputs;
  const outputPrefix = `Worker${definition.id}`;
  expect(outputs[`${outputPrefix}QueueUrl`]).toEqual({ Value: source.url });
  expect(outputs[`${outputPrefix}QueueArn`]).toEqual({ Value: source.arn });
  expect(outputs[`${outputPrefix}DeadLetterQueueUrl`]).toEqual({ Value: dlq.url });
  expect(outputs[`${outputPrefix}DeadLetterQueueArn`]).toEqual({ Value: dlq.arn });

  for (const [kind, actions] of [
    ["publisher", ["sqs:SendMessage", "sqs:GetQueueAttributes"]],
    ["consumer", ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:ChangeMessageVisibility", "sqs:GetQueueAttributes"]],
  ] as const) {
    const policies = Object.entries(template.findResources("AWS::IAM::ManagedPolicy"))
      .filter(([, resource]) => resource.Properties.ManagedPolicyName === `aura-worker-${workerScope}-${kind}-${stage}`);
    expect(policies).toHaveLength(1);
    const [policyId, policy] = policies[0];
    expect(policy.Properties.PolicyDocument).toEqual({
      Version: "2012-10-17",
      Statement: [
        { Effect: "Allow", Action: actions, Resource: source.arn },
        { Effect: "Allow", Action: "sqs:GetQueueAttributes", Resource: dlq.arn },
      ],
    });
    // Exact statements forbid batch pseudo-actions, GetQueueUrl, DLQ mutation, and operator powers.
    expect(policy.Properties.Roles).toBeUndefined();
    expect(policy.Properties.Users).toBeUndefined();
    expect(policy.Properties.Groups).toBeUndefined();
    const suffix = kind === "publisher" ? "PublisherPolicyArn" : "ConsumerPolicyArn";
    expect(outputs[`${outputPrefix}${suffix}`]).toEqual({ Value: { Ref: policyId } });
  }
}

describe.each(STAGES)("%s worker queues", (stage) => {
  let stacks: ReturnType<typeof createApplicationStacks>;
  let data: Template;
  let compute: Template;

  beforeAll(() => {
    const app = new cdk.App({ analyticsReporting: false });
    stacks = createApplicationStacks(app, { stage });
    // CDK's default cycle validation stays enabled, including the DLQ redrive dependency.
    data = Template.fromStack(stacks.data);
    compute = Template.fromStack(stacks.compute);
  });

  test("enables exactly ten scopes, separate from the Shopify catalog", () => {
    expect(WORKER_SCOPES).toEqual(EXPECTED_SCOPES);
    expect(Object.keys(WORKER_QUEUE_DEFINITIONS)).toEqual(EXPECTED_SCOPES);
    expect(stageConfig(stage).workerQueues.enabledScopes).toEqual(EXPECTED_SCOPES);
    expect(Object.keys(stacks.data.workerQueues.catalog)).toEqual(EXPECTED_SCOPES);
    expect(QUEUE_DEFINITIONS).toEqual({ shopify: {
      id: "ShopifyLambda",
      queueName: "shopify-lambda-queue",
      deadLetterQueueName: "shopify-lambda-dlq",
      visibilityTimeoutSeconds: 180,
      maxReceiveCount: 5,
    } });
    const names = Object.values(data.findResources("AWS::SQS::Queue")).map((resource) => resource.Properties.QueueName);
    expect(names.sort()).toEqual([
      ...EXPECTED_SCOPES.flatMap((scope) => [`aura-worker-${scope}-${stage}`, `aura-worker-${scope}-dlq-${stage}`]),
      `shopify-lambda-queue-${stage}`, `shopify-lambda-dlq-${stage}`,
    ].sort());
    expect(new Set(names).size).toBe(22);
    data.resourceCountIs("AWS::IAM::ManagedPolicy", 20);
    data.resourceCountIs("AWS::IAM::User", 0);
    data.resourceCountIs("AWS::IAM::AccessKey", 0);
    data.resourceCountIs("AWS::IAM::Role", 0);
    data.resourceCountIs("AWS::KMS::Key", 0);
    data.resourceCountIs("AWS::Lambda::Function", 0);
  });

  test.each(EXPECTED_SCOPES)("enforces queue, TLS, lifecycle, IAM and output contract for %s", (scope) => {
    expectWorkerPair(stacks.data, data, stage, scope);
  });

  test("exports only deliberate worker handoff values in the effective region/stage", () => {
    const outputs = data.toJSON().Outputs;
    const expectedKeys = EXPECTED_SCOPES.flatMap((scope) => [
      "QueueUrl", "QueueArn", "DeadLetterQueueUrl", "DeadLetterQueueArn", "PublisherPolicyArn", "ConsumerPolicyArn",
    ].map((suffix) => `Worker${EXPECTED_WORKERS[scope].id}${suffix}`));
    expect(Object.keys(outputs).filter((key) => key.startsWith("Worker")).sort())
      .toEqual([...expectedKeys, "WorkerQueueAwsRegion", "WorkerQueueStage"].sort());
    expect(outputs.WorkerQueueAwsRegion).toEqual({ Value: { Ref: "AWS::Region" } });
    expect(outputs.WorkerQueueStage).toEqual({ Value: stage });
    expect(JSON.stringify(compute.toJSON())).not.toContain("aura-worker-");
    expect(JSON.stringify(Template.fromStack(stacks.api).toJSON())).not.toContain("aura-worker-");
  });

  test("keeps Shopify queues, Lambda identity and event wiring unchanged", () => {
    const source = queueResource(data, `shopify-lambda-queue-${stage}`);
    const dlq = queueResource(data, `shopify-lambda-dlq-${stage}`);
    expect(source.id).toBe("QueuesShopifyLambdaQueue117CAC9C");
    expect(dlq.id).toBe("QueuesShopifyLambdaDeadLetterQueue6E6814A0");
    const lifecycle = stage === "prod" ? "Retain" : "Delete";
    expect(source.resource).toEqual({
      Type: "AWS::SQS::Queue",
      Properties: { QueueName: `shopify-lambda-queue-${stage}`, RedrivePolicy: { deadLetterTargetArn: dlq.arn, maxReceiveCount: 5 }, VisibilityTimeout: 180 },
      DeletionPolicy: lifecycle, UpdateReplacePolicy: lifecycle,
    });
    expect(dlq.resource).toEqual({
      Type: "AWS::SQS::Queue",
      Properties: { QueueName: `shopify-lambda-dlq-${stage}`, MessageRetentionPeriod: 1209600 },
      DeletionPolicy: lifecycle, UpdateReplacePolicy: lifecycle,
    });
    expect(data.toJSON().Outputs.ShopifyLambdaQueueUrl).toEqual({ Value: source.url });
    expect(data.toJSON().Outputs.ShopifyLambdaDeadLetterQueueUrl).toEqual({ Value: dlq.url });

    const arn = stacks.compute.resolve(stacks.compute.formatArn({ service: "sqs", resource: `shopify-lambda-queue-${stage}` }));
    const lambda = compute.toJSON().Resources.LambdasShopifyLambda9FCE3162;
    expect(lambda.Properties).toMatchObject({
      FunctionName: `shopify-lambda-${stage}`, Runtime: "provided.al2023", Handler: "lib.handler",
      MemorySize: 256, Timeout: 30, Architectures: ["x86_64"], EphemeralStorage: { Size: 512 },
      Role: { "Fn::GetAtt": ["LambdasShopifyLambdaServiceRoleDDA039B4", "Arn"] },
      Code: {
        S3Bucket: "aura-historia-binary-artifacts-eu-central-1",
        S3Key: { "Fn::Join": ["", [`shopify-lambda-${stage}-`, { Ref: "CommitSHA" }, ".zip"]] },
      },
    });
    expect(Object.keys(lambda.Properties.Environment.Variables).sort()).toEqual([
      "POSTGRES_DATABASE", "POSTGRES_HOST", "POSTGRES_MAX_CONNECTIONS", "POSTGRES_PASSWORD", "POSTGRES_PORT", "POSTGRES_USERNAME",
    ]);
    const policy = compute.toJSON().Resources.LambdasShopifyLambdaServiceRoleDefaultPolicyB8C48B8C;
    expect(policy.Properties.PolicyDocument.Statement).toEqual([{
      Action: ["sqs:ReceiveMessage", "sqs:ChangeMessageVisibility", "sqs:GetQueueUrl", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
      Effect: "Allow", Resource: arn,
    }]);
    expect(policy.Properties.Roles).toEqual([{ Ref: "LambdasShopifyLambdaServiceRoleDDA039B4" }]);
    compute.resourceCountIs("AWS::Lambda::EventSourceMapping", 1);
    compute.hasResourceProperties("AWS::Lambda::EventSourceMapping", {
      EventSourceArn: arn, FunctionName: { Ref: "LambdasShopifyLambda9FCE3162" },
      BatchSize: 10, FunctionResponseTypes: ["ReportBatchItemFailures"], MaximumBatchingWindowInSeconds: 1,
    });
    const rule = compute.toJSON().Resources.EventingShopifyEventRule401F6A4E;
    expect(rule.Properties.EventPattern).toEqual({ detail: { metadata: {
      "X-Shopify-Topic": ["products/create", "products/update", "products/delete"],
    } } });
    expect(rule.Properties.Targets).toEqual([{ Arn: arn, Id: "Target0" }]);
    expect(compute.toJSON().Resources.EventingShopifyEventRuleQueuePolicy86C4784B.Properties.PolicyDocument.Statement).toEqual([{
      Effect: "Allow", Principal: { Service: "events.amazonaws.com" }, Action: "sqs:SendMessage", Resource: arn,
      Condition: { ArnEquals: { "aws:SourceArn": { "Fn::GetAtt": ["EventingShopifyEventRule401F6A4E", "Arn"] } } },
    }]);
  });

  test("uses prod-only age and DLQ backlog alarms on the existing SNS topic", () => {
    data.resourceCountIs("AWS::CloudWatch::Alarm", 0);
    compute.resourceCountIs("AWS::CloudWatch::Alarm", 0);
    if (stage !== "prod") {
      expect(stacks.observability).toBeUndefined();
      return;
    }
    const template = Template.fromStack(stacks.observability!);
    const alarms = Object.values(template.findResources("AWS::CloudWatch::Alarm"))
      .filter((resource) => resource.Properties.Namespace === "AWS/SQS");
    expect(alarms).toHaveLength(20);
    const topicIds = Object.keys(template.findResources("AWS::SNS::Topic"));
    expect(topicIds).toHaveLength(1);
    for (const scope of EXPECTED_SCOPES) {
      for (const [suffix, metricName, queueName, threshold] of [
        ["source-age", "ApproximateAgeOfOldestMessage", `aura-worker-${scope}-prod`, 900],
        ["dlq-visible", "ApproximateNumberOfMessagesVisible", `aura-worker-${scope}-dlq-prod`, 1],
      ] as const) {
        template.hasResourceProperties("AWS::CloudWatch::Alarm", {
          AlarmName: `prod-worker-${scope}-${suffix}`, Namespace: "AWS/SQS", MetricName: metricName,
          Dimensions: [{ Name: "QueueName", Value: queueName }], Statistic: "Maximum", Period: 300,
          Threshold: threshold, EvaluationPeriods: 1, ComparisonOperator: "GreaterThanOrEqualToThreshold",
          TreatMissingData: "notBreaching", AlarmActions: [{ Ref: topicIds[0] }],
        });
      }
    }
  });
});

test("single-stack ephemeral has the same complete worker contract, with no worker Lambda wiring", () => {
  const app = new cdk.App({ analyticsReporting: false });
  const stack = new ApplicationEphemeralStack(app, "application-ephemeral", { stage: "ephemeral" });
  const template = Template.fromStack(stack);
  for (const scope of EXPECTED_SCOPES) {
    expectWorkerPair(stack, template, "ephemeral", scope);
  }
  template.resourceCountIs("AWS::SQS::Queue", 22);
  template.resourceCountIs("AWS::IAM::ManagedPolicy", 20);
  template.resourceCountIs("AWS::IAM::User", 0);
  template.resourceCountIs("AWS::IAM::AccessKey", 0);
  template.resourceCountIs("AWS::CloudWatch::Alarm", 0);
  template.resourceCountIs("AWS::Lambda::EventSourceMapping", 1);
  template.hasResourceProperties("AWS::Lambda::EventSourceMapping", {
    EventSourceArn: { "Fn::GetAtt": ["QueuesShopifyLambdaQueue117CAC9C", "Arn"] },
    FunctionName: { Ref: "LambdasShopifyLambda9FCE3162" },
    BatchSize: 10, FunctionResponseTypes: ["ReportBatchItemFailures"], MaximumBatchingWindowInSeconds: 1,
  });
  expect(template.toJSON().Outputs.WorkerQueueStage.Value).toBe("ephemeral");
});

test.each<{ enabledScopes: WorkerScope[] }>([
  { enabledScopes: [] },
  { enabledScopes: ["product-embedding"] },
])("only enabled scopes get resources, imports and outputs: $enabledScopes", ({ enabledScopes }) => {
  const app = new cdk.App({ analyticsReporting: false });
  const stack = new cdk.Stack(app, "selected-workers");
  const config = stageConfig("dev");
  const selected = { ...config, workerQueues: { ...config.workerQueues, enabledScopes } };
  const queues = new WorkerQueues(stack, "WorkerQueues", { config: selected });
  queues.addOutputs();
  const imports = importWorkerQueueCatalog(stack, "Imports", selected);
  const template = Template.fromStack(stack);
  expect(Object.keys(queues.catalog)).toEqual(enabledScopes);
  expect(Object.keys(imports)).toEqual(enabledScopes);
  template.resourceCountIs("AWS::SQS::Queue", enabledScopes.length * 2);
  template.resourceCountIs("AWS::IAM::ManagedPolicy", enabledScopes.length * 2);
  expect(Object.keys(template.toJSON().Outputs)).toHaveLength(enabledScopes.length * 6 + 2);
});

test("example uses the exact source queue, region, stage and scope without credentials or endpoint overrides", () => {
  const example = fs.readFileSync(path.join(__dirname, "../examples/worker.env.example"), "utf8");
  const environment = Object.fromEntries(example.split("\n")
    .filter((line) => line.trim() && !line.startsWith("#"))
    .map((line) => line.split("=")));
  expect(environment).toEqual({
    AWS_REGION: "eu-central-1",
    STAGE: "prod",
    AURA_HISTORIA_WORKER_SCOPE: "notification-delivery",
    AURA_HISTORIA_WORKER_QUEUE_URL: "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-notification-delivery-prod",
  });
});

test("queue names use stage, never a custom stack prefix, and reject names over SQS's limit", () => {
  const app = new cdk.App({ analyticsReporting: false });
  const stack = new ApplicationDataStack(app, "custom-task-prefix-data", {
    stage: "dev", env: { account: "123456789012", region: "eu-central-1" },
  });
  const template = Template.fromStack(stack);
  const names = Object.values(template.findResources("AWS::SQS::Queue")).map((resource) => resource.Properties.QueueName);
  expect(names.every((name) => name.endsWith("-dev"))).toBe(true);
  expect(template.toJSON().Outputs.WorkerQueueAwsRegion.Value).toBe("eu-central-1");
  for (const stage of STAGES) {
    for (const scope of EXPECTED_SCOPES) {
      expect(workerQueueName(scope, stage)).toBe(`aura-worker-${scope}-${stage}`);
      expect(workerQueueName(scope, stage, true)).toBe(`aura-worker-${scope}-dlq-${stage}`);
      expect(workerQueueName(scope, stage, true).length).toBeLessThanOrEqual(80);
    }
  }
  expect(() => workerQueueName("notification-delivery", "x".repeat(80) as StageName, true)).toThrow("at most 80");
  expect(() => workerQueueName("notification-delivery", "invalid_stage" as StageName)).toThrow("Invalid Standard worker queue name");
});

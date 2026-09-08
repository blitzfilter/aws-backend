import * as cdk from "aws-cdk-lib";
import * as iam from "aws-cdk-lib/aws-iam";
import * as sqs from "aws-cdk-lib/aws-sqs";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { WORKER_QUEUE_DEFINITIONS, workerQueueName, type WorkerScope } from "../worker-queue-config";

export interface WorkerQueuePair {
  readonly queue: sqs.IQueue;
  readonly deadLetterQueue: sqs.IQueue;
}

export type WorkerQueueCatalog = Partial<Record<WorkerScope, WorkerQueuePair>>;

interface WorkerQueueResources extends WorkerQueuePair {
  readonly publisherPolicy: iam.IManagedPolicy;
  readonly consumerPolicy: iam.IManagedPolicy;
}

export interface WorkerQueuesProps {
  readonly config: StageConfig;
}

export class WorkerQueues extends Construct {
  readonly catalog: Partial<Record<WorkerScope, WorkerQueueResources>> = {};

  constructor(scope: Construct, id: string, private readonly props: WorkerQueuesProps) {
    super(scope, id);

    const { config } = props;
    const settings = config.workerQueues;
    for (const workerScope of settings.enabledScopes) {
      const definition = WORKER_QUEUE_DEFINITIONS[workerScope];
      const queueName = workerQueueName(workerScope, config.stage);
      // A literal-name import avoids a source -> DLQ -> source CloudFormation cycle.
      const sourceForRedrive = importWorkerQueue(this, `${definition.id}RedriveSource`, queueName);
      const deadLetterQueue = new sqs.Queue(this, `${definition.id}DeadLetterQueue`, {
        queueName: workerQueueName(workerScope, config.stage, true),
        fifo: false,
        retentionPeriod: cdk.Duration.days(settings.deadLetterRetentionDays),
        receiveMessageWaitTime: cdk.Duration.seconds(settings.receiveWaitTimeSeconds),
        encryption: sqs.QueueEncryption.SQS_MANAGED,
        enforceSSL: true,
        redriveAllowPolicy: {
          redrivePermission: sqs.RedrivePermission.BY_QUEUE,
          sourceQueues: [sourceForRedrive],
        },
        removalPolicy: config.removalPolicy,
      });
      const queue = new sqs.Queue(this, `${definition.id}Queue`, {
        queueName,
        fifo: false,
        retentionPeriod: cdk.Duration.days(settings.sourceRetentionDays),
        visibilityTimeout: cdk.Duration.seconds(definition.visibilityTimeoutSeconds),
        receiveMessageWaitTime: cdk.Duration.seconds(settings.receiveWaitTimeSeconds),
        encryption: sqs.QueueEncryption.SQS_MANAGED,
        enforceSSL: true,
        deadLetterQueue: { queue: deadLetterQueue, maxReceiveCount: settings.maxReceiveCount },
        redriveAllowPolicy: { redrivePermission: sqs.RedrivePermission.DENY_ALL },
        removalPolicy: config.removalPolicy,
      });

      // The bare-metal identity is externally owned. Export policies, never create credentials
      // or bind them to an unrelated Lambda/CI role. Startup inspects the paired DLQ as well.
      const publisherPolicy = new iam.ManagedPolicy(this, `${definition.id}PublisherPolicy`, {
        managedPolicyName: `aura-worker-${workerScope}-publisher-${config.stage}`,
        description: `Publish ${workerScope} jobs and validate its queue pair (${config.stage})`,
        statements: [
          new iam.PolicyStatement({
            actions: ["sqs:SendMessage", "sqs:GetQueueAttributes"],
            resources: [queue.queueArn],
          }),
          new iam.PolicyStatement({ actions: ["sqs:GetQueueAttributes"], resources: [deadLetterQueue.queueArn] }),
        ],
      });
      const consumerPolicy = new iam.ManagedPolicy(this, `${definition.id}ConsumerPolicy`, {
        managedPolicyName: `aura-worker-${workerScope}-consumer-${config.stage}`,
        description: `Consume ${workerScope} jobs and validate its queue pair (${config.stage})`,
        statements: [
          new iam.PolicyStatement({
            actions: ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:ChangeMessageVisibility", "sqs:GetQueueAttributes"],
            resources: [queue.queueArn],
          }),
          new iam.PolicyStatement({ actions: ["sqs:GetQueueAttributes"], resources: [deadLetterQueue.queueArn] }),
        ],
      });
      this.catalog[workerScope] = { queue, deadLetterQueue, publisherPolicy, consumerPolicy };
    }
  }

  addOutputs(): void {
    const stack = cdk.Stack.of(this);
    new cdk.CfnOutput(stack, "WorkerQueueAwsRegion", { value: stack.region });
    new cdk.CfnOutput(stack, "WorkerQueueStage", { value: this.props.config.stage });
    for (const workerScope of this.props.config.workerQueues.enabledScopes) {
      const resources = this.catalog[workerScope]!;
      const prefix = `Worker${WORKER_QUEUE_DEFINITIONS[workerScope].id}`;
      const outputs = {
        QueueUrl: resources.queue.queueUrl,
        QueueArn: resources.queue.queueArn,
        DeadLetterQueueUrl: resources.deadLetterQueue.queueUrl,
        DeadLetterQueueArn: resources.deadLetterQueue.queueArn,
        PublisherPolicyArn: resources.publisherPolicy.managedPolicyArn,
        ConsumerPolicyArn: resources.consumerPolicy.managedPolicyArn,
      };
      for (const [suffix, value] of Object.entries(outputs)) {
        new cdk.CfnOutput(stack, `${prefix}${suffix}`, { value });
      }
    }
  }
}

export function importWorkerQueueCatalog(scope: Construct, id: string, config: StageConfig): WorkerQueueCatalog {
  const importScope = new Construct(scope, id);
  const catalog: WorkerQueueCatalog = {};
  for (const workerScope of config.workerQueues.enabledScopes) {
    const definition = WORKER_QUEUE_DEFINITIONS[workerScope];
    catalog[workerScope] = {
      queue: importWorkerQueue(importScope, `${definition.id}Queue`, workerQueueName(workerScope, config.stage)),
      deadLetterQueue: importWorkerQueue(importScope, `${definition.id}DeadLetterQueue`, workerQueueName(workerScope, config.stage, true)),
    };
  }
  return catalog;
}

function importWorkerQueue(scope: Construct, id: string, queueName: string): sqs.IQueue {
  return sqs.Queue.fromQueueAttributes(scope, id, {
    queueName,
    queueArn: cdk.Stack.of(scope).formatArn({ service: "sqs", resource: queueName }),
    queueUrl: cdk.Fn.sub("https://sqs.${AWS::Region}.${AWS::URLSuffix}/${AWS::AccountId}/${QueueName}", {
      QueueName: queueName,
    }),
  });
}

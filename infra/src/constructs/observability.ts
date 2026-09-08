import * as cdk from "aws-cdk-lib";
import * as apigwv2 from "aws-cdk-lib/aws-apigatewayv2";
import * as cloudwatch from "aws-cdk-lib/aws-cloudwatch";
import * as actions from "aws-cdk-lib/aws-cloudwatch-actions";
import * as sns from "aws-cdk-lib/aws-sns";
import { Construct } from "constructs";
import type { StageConfig } from "../config";
import { lambdaFunctionName, type LambdaCatalog, type LambdaKey } from "./lambdas";
import { WORKER_QUEUE_DEFINITIONS } from "../worker-queue-config";
import type { WorkerQueueCatalog } from "./worker-queues";

export interface ObservabilityProps {
  readonly config: StageConfig;
  readonly stageName: string;
  readonly api: apigwv2.HttpApi;
  readonly functions: LambdaCatalog;
  readonly workerQueues: WorkerQueueCatalog;
}

export class Observability extends Construct {
  readonly alarmTopic: sns.Topic | undefined;

  constructor(scope: Construct, id: string, props: ObservabilityProps) {
    super(scope, id);

    if (!props.config.enableProductionObservability) {
      return;
    }

    this.alarmTopic = new sns.Topic(this, "AlarmNotificationTopic", {
      topicName: `cloudwatch-alarms-${props.stageName}`,
      displayName: `CloudWatch Alarms for Aura-Historia Backend stage '${props.stageName}'`,
    });
    const alarmAction = new actions.SnsAction(this.alarmTopic);

    const settings = props.config.workerQueues.alarms;
    for (const workerScope of props.config.workerQueues.enabledScopes) {
      const queues = props.workerQueues[workerScope];
      if (!queues) {
        throw new Error(`Missing alarm queues for enabled worker scope '${workerScope}'.`);
      }
      const definition = WORKER_QUEUE_DEFINITIONS[workerScope];
      const metricOptions = { statistic: "Maximum", period: cdk.Duration.seconds(settings.periodSeconds) };
      const alarmOptions = {
        evaluationPeriods: settings.evaluationPeriods,
        comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
        treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
      };
      new cloudwatch.Alarm(this, `Worker${definition.id}SourceAgeAlarm`, {
        ...alarmOptions,
        alarmName: `${props.stageName}-worker-${workerScope}-source-age`,
        alarmDescription: `Oldest ${workerScope} job is at least ${settings.sourceAgeThresholdSeconds}s old; check consumer health and dependencies.`,
        metric: queues.queue.metricApproximateAgeOfOldestMessage(metricOptions),
        threshold: settings.sourceAgeThresholdSeconds,
      }).addAlarmAction(alarmAction);
      new cloudwatch.Alarm(this, `Worker${definition.id}DeadLetterVisibleAlarm`, {
        ...alarmOptions,
        alarmName: `${props.stageName}-worker-${workerScope}-dlq-visible`,
        alarmDescription: `${workerScope} has dead-letter jobs; investigate before operator-controlled replay.`,
        metric: queues.deadLetterQueue.metricApproximateNumberOfMessagesVisible(metricOptions),
        threshold: settings.deadLetterVisibleThreshold,
      }).addAlarmAction(alarmAction);
    }

    apiAlarm(this, props.stageName, "Api4XXErrorAlarm", "4XXError", props.api, 50, 2, "Sum").addAlarmAction(alarmAction);
    apiAlarm(this, props.stageName, "Api5XXErrorAlarm", "5XXError", props.api, 5, 1, "Sum").addAlarmAction(alarmAction);
    apiAlarm(this, props.stageName, "ApiLatencyAlarm", "IntegrationLatency", props.api, 3000, 2, "Average").addAlarmAction(alarmAction);

    lambdaAlarm(
      this,
      props.stageName,
      "InitialFxRateSnapshotProviderErrorAlarm",
      "Errors",
      `fxrate-initial-snapshot-provider-${props.stageName}`,
      1,
    ).addAlarmAction(alarmAction);

    for (const [key, fn] of Object.entries(props.functions) as [LambdaKey, unknown][]) {
      if (!fn) {
        continue;
      }

      const functionName = lambdaFunctionName(key, props.config.stage);
      lambdaAlarm(this, props.stageName, `${key}ErrorAlarm`, "Errors", functionName, queueWorkerKeys.has(key) ? 5 : 1)
        .addAlarmAction(alarmAction);

      if (apiLambdaKeys.has(key)) {
        lambdaAlarm(this, props.stageName, `${key}ThrottleAlarm`, "Throttles", functionName, 1).addAlarmAction(alarmAction);
      }
    }
  }
}

function lambdaAlarm(
  scope: Construct,
  stageName: string,
  id: string,
  metricName: string,
  functionName: string,
  threshold: number,
): cloudwatch.Alarm {
  return new cloudwatch.Alarm(scope, id, {
    alarmName: `${stageName}-${toKebabCase(functionName)}-${toKebabCase(metricName)}`,
    alarmDescription: `Alarm when ${functionName} has Lambda ${metricName}`,
    metric: new cloudwatch.Metric({
      namespace: "AWS/Lambda",
      metricName,
      dimensionsMap: { FunctionName: functionName },
      statistic: "Sum",
      period: cdk.Duration.minutes(5),
    }),
    threshold,
    evaluationPeriods: 1,
    comparisonOperator: cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
    treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
  });
}

function apiAlarm(
  scope: Construct,
  stageName: string,
  id: string,
  metricName: string,
  api: apigwv2.HttpApi,
  threshold: number,
  evaluationPeriods: number,
  statistic: string,
): cloudwatch.Alarm {
  return new cloudwatch.Alarm(scope, id, {
    alarmName: `${stageName}-api-${toKebabCase(metricName)}`,
    metric: new cloudwatch.Metric({
      namespace: "AWS/ApiGateway",
      metricName,
      dimensionsMap: { ApiId: api.apiId },
      statistic,
      period: cdk.Duration.minutes(5),
    }),
    threshold,
    evaluationPeriods,
    comparisonOperator:
      metricName === "IntegrationLatency"
        ? cloudwatch.ComparisonOperator.GREATER_THAN_THRESHOLD
        : cloudwatch.ComparisonOperator.GREATER_THAN_OR_EQUAL_TO_THRESHOLD,
    treatMissingData: cloudwatch.TreatMissingData.NOT_BREACHING,
  });
}

const apiLambdaKeys = new Set<LambdaKey>();

const queueWorkerKeys = new Set<LambdaKey>(["shopify"]);

function toKebabCase(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
    .replace(/[^a-zA-Z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .toLowerCase();
}

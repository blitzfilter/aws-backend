import * as cdk from "aws-cdk-lib";
import * as s3 from "aws-cdk-lib/aws-s3";
import { Construct } from "constructs";
import {
  ARTIFACT_BUCKET_NAME,
  CLOUDFORMATION_STAGING_BUCKET_NAME,
  MAIL_TEMPLATE_BUCKET_NAME,
  type StageName,
} from "./config";
import { stageConfig } from "./config";
import { applicationParameters } from "./parameters";
import { BackendHttpApi } from "./constructs/api";
import { Identity } from "./constructs/cognito";
import { Eventing } from "./constructs/eventing";
import { addUserPoolEnvironment, grantCognitoAdminAccess, importLambdaCatalog, Lambdas } from "./constructs/lambdas";
import { Observability } from "./constructs/observability";
import { Search } from "./constructs/opensearch";
import { importQueueCatalog, Queues } from "./constructs/queues";
import { Storage } from "./constructs/storage";

export interface ApplicationStackProps extends cdk.StackProps {
  readonly stage: StageName;
  readonly localStackMappedPort?: string;
}

export interface ApplicationStageProps extends cdk.StackProps {
  readonly stage: StageName;
  readonly stackNamePrefix?: string;
  readonly localStackMappedPort?: string;
}

export interface ApplicationStageStacks {
  readonly data: ApplicationDataStack;
  readonly compute: ApplicationComputeStack;
  readonly api: ApplicationApiStack;
  readonly observability?: ApplicationObservabilityStack;
}

export function createApplicationStacks(scope: Construct, props: ApplicationStageProps): ApplicationStageStacks {
  const stackNamePrefix = props.stackNamePrefix ?? `application-${props.stage}`;
  const baseProps = stackBaseProps(props);

  const data = new ApplicationDataStack(scope, `${stackNamePrefix}-data`, {
    ...baseProps,
    stage: props.stage,
    localStackMappedPort: props.localStackMappedPort,
    stackName: `${stackNamePrefix}-data`,
  });

  const compute = new ApplicationComputeStack(scope, `${stackNamePrefix}-compute`, {
    ...baseProps,
    stage: props.stage,
    localStackMappedPort: props.localStackMappedPort,
    stackName: `${stackNamePrefix}-compute`,
    storage: data.storage,
    queues: data.queues,
    search: data.search,
  });
  compute.addDependency(data);

  const api = new ApplicationApiStack(scope, `${stackNamePrefix}-api`, {
    ...baseProps,
    stage: props.stage,
    localStackMappedPort: props.localStackMappedPort,
    stackName: `${stackNamePrefix}-api`,
    identity: compute.identity,
  });
  api.addDependency(compute);

  const observability = props.stage === "prod"
    ? new ApplicationObservabilityStack(scope, `${stackNamePrefix}-observability`, {
        ...baseProps,
        stage: props.stage,
        localStackMappedPort: props.localStackMappedPort,
        stackName: `${stackNamePrefix}-observability`,
        api: api.api,
      })
    : undefined;
  observability?.addDependency(api);
  observability?.addDependency(data);
  observability?.addDependency(compute);

  return {
    data,
    compute,
    api,
    observability,
  };
}

export class ApplicationDataStack extends cdk.Stack {
  readonly storage: Storage;
  readonly queues: Queues;
  readonly search: Search;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia data stack";

    this.storage = new Storage(this, "Storage", {
      config,
    });

    this.queues = new Queues(this, "Queues", {
      config,
      stageName,
    });

    this.search = new Search(this, "Search", {
      config,
    });

    dataOutputs(this, {
      storage: this.storage,
      queues: this.queues,
      search: this.search,
    });
  }
}

export interface ApplicationComputeStackProps extends ApplicationStackProps {
  readonly storage: Storage;
  readonly queues: Queues;
  readonly search: Search;
}

export class ApplicationComputeStack extends cdk.Stack {
  readonly lambdas: Lambdas;
  readonly identity: Identity;
  readonly eventing: Eventing;

  constructor(scope: Construct, id: string, props: ApplicationComputeStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const parameters = applicationParameters(this);
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia compute stack";

    const artifactBucket = s3.Bucket.fromBucketName(this, "ArtifactBucketImport", ARTIFACT_BUCKET_NAME);
    const mailTemplateBucket = s3.Bucket.fromBucketName(this, "MailTemplateBucketImport", MAIL_TEMPLATE_BUCKET_NAME);

    this.lambdas = new Lambdas(this, "Lambdas", {
      config,
      parameters,
      artifactBucket,
      mailTemplateBucket,
      postgres: props.storage.postgres,
    });


    this.identity = new Identity(this, "Identity", {
      config,
      stageName,
      postConfirmationLambda: this.lambdas.functions.postConfirmation,
    });
    addUserPoolEnvironment(
      this.lambdas.functions,
      this.identity.userPool.userPoolId,
      this.identity.publicClient.userPoolClientId,
    );
    grantCognitoAdminAccess(this.lambdas.functions, this.identity.userPool.userPoolArn);

    this.eventing = new Eventing(this, "Eventing", {
      config,
      queues: importQueueCatalog(this, "EventingQueueImports", stageName),
      functions: this.lambdas.functions,
    });

    computeOutputs(this, {
      identity: this.identity,
      eventing: this.eventing,
    });
  }
}

export interface ApplicationApiStackProps extends ApplicationStackProps {
  readonly identity: Identity;
}

export class ApplicationApiStack extends cdk.Stack {
  readonly api: BackendHttpApi;

  constructor(scope: Construct, id: string, props: ApplicationApiStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia API stack";

    this.api = new BackendHttpApi(this, "HttpApi", {
      config,
      stageName,
      functions: importLambdaCatalog(this, "LambdaImports", config),
      identity: props.identity,
    });

    new cdk.CfnOutput(this, "ApiGatewayEndpointUrl", { value: this.api.endpointUrl });
    if (this.api.distribution) {
      new cdk.CfnOutput(this, "ApiCloudFrontDistributionDomainName", {
        value: this.api.distribution.attrDomainName,
      });
    }
  }
}

export class ApplicationEphemeralStack extends cdk.Stack {
  readonly storage: Storage;
  readonly queues: Queues;
  readonly search: Search;
  readonly lambdas: Lambdas;
  readonly identity: Identity;
  readonly eventing: Eventing;
  readonly api: BackendHttpApi;

  constructor(scope: Construct, id: string, props: ApplicationStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    if (!config.isEphemeral) {
      throw new Error("ApplicationEphemeralStack only supports the ephemeral stage.");
    }

    const parameters = applicationParameters(this);
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia ephemeral acceptance-test stack";

    this.storage = new Storage(this, "Storage", {
      config,
    });
    this.queues = new Queues(this, "Queues", {
      config,
      stageName,
    });
    this.search = new Search(this, "Search", {
      config,
    });

    const artifactBucket = s3.Bucket.fromBucketName(this, "ArtifactBucketImport", ARTIFACT_BUCKET_NAME);
    const mailTemplateBucket = s3.Bucket.fromBucketName(this, "MailTemplateBucketImport", MAIL_TEMPLATE_BUCKET_NAME);

    this.lambdas = new Lambdas(this, "Lambdas", {
      config,
      parameters,
      artifactBucket,
      mailTemplateBucket,
      postgres: this.storage.postgres,
    });


    this.identity = new Identity(this, "Identity", {
      config,
      stageName,
      postConfirmationLambda: this.lambdas.functions.postConfirmation,
    });
    addUserPoolEnvironment(
      this.lambdas.functions,
      this.identity.userPool.userPoolId,
      this.identity.publicClient.userPoolClientId,
    );
    grantCognitoAdminAccess(this.lambdas.functions, this.identity.userPool.userPoolArn);

    this.eventing = new Eventing(this, "Eventing", {
      config,
      queues: this.queues.catalog,
      functions: this.lambdas.functions,
    });

    this.api = new BackendHttpApi(this, "HttpApi", {
      config,
      stageName,
      functions: this.lambdas.functions,
      identity: this.identity,
    });

    dataOutputs(this, {
      storage: this.storage,
      queues: this.queues,
      search: this.search,
    });
    computeOutputs(this, {
      identity: this.identity,
      eventing: this.eventing,
    });
    new cdk.CfnOutput(this, "ApiGatewayEndpointUrl", { value: this.api.endpointUrl });
  }
}

export interface ApplicationObservabilityStackProps extends ApplicationStackProps {
  readonly api: BackendHttpApi;
}

export class ApplicationObservabilityStack extends cdk.Stack {
  readonly observability: Observability;

  constructor(scope: Construct, id: string, props: ApplicationObservabilityStackProps) {
    super(scope, id, stackProps(props));

    const config = stageConfig(props.stage, {
      localStackMappedPort: props.localStackMappedPort,
    });
    const stageName = config.stage;

    this.templateOptions.description = "Aura Historia observability stack";

    this.observability = new Observability(this, "Observability", {
      config,
      stageName,
      api: props.api.api,
      functions: importLambdaCatalog(this, "LambdaAlarmImports", config),
    });

    if (this.observability.alarmTopic) {
      new cdk.CfnOutput(this, "AlarmNotificationTopicArn", {
        description: "SNS Topic ARN for CloudWatch alarm notifications",
        value: this.observability.alarmTopic.topicArn,
      });
    }
  }
}

function stackBaseProps(props: ApplicationStageProps): cdk.StackProps {
  const { localStackMappedPort: _localStackMappedPort, stackNamePrefix: _stackNamePrefix, stage: _stage, ...stackProps } = props;
  return stackProps;
}

function stackProps(props: ApplicationStackProps): cdk.StackProps {
  return {
    ...stackBaseProps(props),
    synthesizer: new cdk.CliCredentialsStackSynthesizer({
      fileAssetsBucketName: CLOUDFORMATION_STAGING_BUCKET_NAME,
      bucketPrefix: `${props.stage}/`,
    }),
  };
}

function dataOutputs(
  stack: cdk.Stack,
  resources: {
    readonly search: Search;
    readonly storage: Storage;
    readonly queues: Queues;
  },
): void {
  new cdk.CfnOutput(stack, "PostgresHost", { value: resources.storage.postgres.host });
  new cdk.CfnOutput(stack, "PostgresPort", { value: resources.storage.postgres.port });
  new cdk.CfnOutput(stack, "PostgresDatabase", { value: resources.storage.postgres.database });
  new cdk.CfnOutput(stack, "OpensearchDomainName", { value: resources.search.domainName });
  new cdk.CfnOutput(stack, "OutputOpensearchEndpointUrl", {
    key: "OpensearchEndpointUrl",
    value: resources.search.endpointUrl,
  });




  new cdk.CfnOutput(stack, "ShopifyLambdaQueueUrl", {
    value: resources.queues.catalog.shopify.queue.queueUrl,
  });
  new cdk.CfnOutput(stack, "ShopifyLambdaDeadLetterQueueUrl", {
    value: resources.queues.catalog.shopify.deadLetterQueue.queueUrl,
  });

}

function computeOutputs(
  stack: cdk.Stack,
  resources: {
    readonly identity: Identity;
    readonly eventing: Eventing;
  },
): void {
  new cdk.CfnOutput(stack, "CognitoHostedUIDomain", {
    value: cdk.Fn.sub("https://${Domain}.auth.${AWS::Region}.amazoncognito.com", {
      Domain: resources.identity.domain.domainName,
    }),
  });
  new cdk.CfnOutput(stack, "CognitoUserPoolId", { value: resources.identity.userPool.userPoolId });
  new cdk.CfnOutput(stack, "CognitoUserPoolClientPublicId", {
    value: resources.identity.publicClient.userPoolClientId,
  });

  new cdk.CfnOutput(stack, "OutputStripeEventBusName", {
    key: "StripeEventBusName",
    value: resources.eventing.stripeEventBus.eventBusName,
  });
  new cdk.CfnOutput(stack, "OutputShopifyEventBusName", {
    key: "ShopifyEventBusName",
    value: resources.eventing.shopifyEventBus.eventBusName,
  });
}

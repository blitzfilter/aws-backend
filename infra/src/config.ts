import * as cdk from "aws-cdk-lib";
import { WORKER_QUEUE_SETTINGS, type WorkerQueueSettings } from "./worker-queue-config";

export const STAGES = ["prod", "dev", "ephemeral"] as const;
export type StageName = (typeof STAGES)[number];

export const ARTIFACT_BUCKET_NAME = "aura-historia-binary-artifacts-eu-central-1";
export const MAIL_TEMPLATE_BUCKET_NAME = "aura-historia-mail-templates-eu-central-1";
export const CLOUDFORMATION_STAGING_BUCKET_NAME = "aura-historia-cfn-artifcats-eu-central-1";

const LOCALHOST_CALLBACK_URL = "http://localhost:3000";
const STAGE_FRONTEND_URL = "https://stage.aura-historia.com/";
const PROD_FRONTEND_URL = "https://aura-historia.com/";

const PROD_API_CORS_ALLOW_ORIGINS = [
  "https://aura-historia.com",
  "https://admin.shopify.com",
  "https://partners.shopify.com",
  "https://shopify.com",
  "https://*.myshopify.com",
] as const;

export interface CognitoEmailConfig {
  readonly configurationSet: string;
  readonly from: string;
  readonly identityDomain: string;
  readonly replyTo: string;
}

export interface StageConfig {
  readonly stage: StageName;
  readonly isProd: boolean;
  readonly isEphemeral: boolean;
  readonly removalPolicy: cdk.RemovalPolicy;
  readonly workerQueues: WorkerQueueSettings;
  readonly apiEndpointUrl: string | undefined;
  readonly apiDomainName: string | undefined;
  readonly apiGatewayCertificateArn: string | undefined;
  readonly apiCloudFrontCertificateArn: string | undefined;
  readonly apiCloudFrontAliases: string[];
  readonly apiCorsAllowOrigins: string[];
  readonly cognitoCallbackUrls: string[];
  readonly cognitoLogoutUrls: string[];
  readonly cognitoEmail: CognitoEmailConfig | undefined;
  readonly opensearchDomainName: string;
  readonly opensearchEndpointUrl: string;
  readonly enableProductionObservability: boolean;
  readonly stripeCheckoutCancelUrl: string;
  readonly stripeCheckoutSuccessUrl: string;
  readonly stripePortalReturnUrl: string;
  readonly stripeEventBusName: string;
  readonly shopifyEventBusName: string;
  readonly stripeProProductId: string;
  readonly stripeUltimateProductId: string;
  readonly stripeProMonthlyPriceId: string;
  readonly stripeProYearlyPriceId: string;
  readonly stripeUltimateMonthlyPriceId: string;
  readonly stripeUltimateYearlyPriceId: string;
  readonly localStackMappedPort: string;
}

export interface StageConfigOptions {
  readonly localStackMappedPort?: string;
}

export function isStageName(value: string): value is StageName {
  return (STAGES as readonly string[]).includes(value);
}

export function stageConfig(stage: StageName, options: StageConfigOptions = {}): StageConfig {
  const isProd = stage === "prod";
  const isEphemeral = stage === "ephemeral";

  const apiDomainName = stage === "prod" ? "api.aura-historia.com" : stage === "dev" ? "api.dev.aura-historia.com" : undefined;
  const apiCloudFrontAliases = stage === "prod" ? ["api.aura-historia.com"] : stage === "dev" ? ["*.dev.aura-historia.com"] : [];

  return {
    stage,
    isProd,
    isEphemeral,
    removalPolicy: isProd ? cdk.RemovalPolicy.RETAIN : cdk.RemovalPolicy.DESTROY,
    workerQueues: WORKER_QUEUE_SETTINGS,
    apiEndpointUrl: apiDomainName ? `https://${apiDomainName}` : undefined,
    apiDomainName,
    apiGatewayCertificateArn: isEphemeral ? undefined : ssmValue(`/certificates/${stage}/api-regional-certificate-arn`),
    apiCloudFrontCertificateArn: isEphemeral ? undefined : ssmValue(`/certificates/${stage}/api-cloudfront-certificate-arn`),
    apiCloudFrontAliases,
    apiCorsAllowOrigins: isProd ? [...PROD_API_CORS_ALLOW_ORIGINS] : ["*"],
    cognitoCallbackUrls:
      stage === "prod"
        ? [PROD_FRONTEND_URL]
        : stage === "dev"
          ? [LOCALHOST_CALLBACK_URL, STAGE_FRONTEND_URL]
          : [LOCALHOST_CALLBACK_URL],
    cognitoLogoutUrls:
      stage === "prod"
        ? [PROD_FRONTEND_URL]
        : stage === "dev"
          ? [LOCALHOST_CALLBACK_URL, STAGE_FRONTEND_URL]
          : [LOCALHOST_CALLBACK_URL],
    cognitoEmail: isEphemeral
      ? undefined
      : {
          configurationSet: "my-first-configuration-set",
          from: "Aura Historia <auth@notify.aura-historia.com>",
          identityDomain: "notify.aura-historia.com",
          replyTo: "contact@aura-historia.com",
        },
    opensearchDomainName: isEphemeral ? "test-domain" : `aura-historia-${stage}`,
    opensearchEndpointUrl: isEphemeral ? "" : ssmValue(`/opensearch/${stage}/endpoint-url`),
    enableProductionObservability: isProd,
    stripeCheckoutCancelUrl: isProd ? "https://aura-historia.com" : "https://stage.aura-historia.com",
    stripeCheckoutSuccessUrl: isProd
      ? "https://aura-historia.com/me/account"
      : "https://stage.aura-historia.com/me/account",
    stripePortalReturnUrl: isProd
      ? "https://aura-historia.com/me/account"
      : "https://stage.aura-historia.com/me/account",
    stripeEventBusName: isEphemeral
      ? "stripe-event-bus-ephemeral"
      : ssmValue(`/eventbridge/${stage}/stripe-event-bus-name`),
    shopifyEventBusName: isEphemeral
      ? "shopify-event-bus-ephemeral"
      : ssmValue(`/eventbridge/${stage}/shopify-event-bus-name`),
    stripeProProductId: isEphemeral ? "prod_test_pro" : ssmValue(`/stripe/${stage}/pro-product-id`),
    stripeUltimateProductId: isEphemeral ? "prod_test_ultimate" : ssmValue(`/stripe/${stage}/ultimate-product-id`),
    stripeProMonthlyPriceId: isEphemeral ? "price_pro_monthly_mock" : ssmValue(`/stripe/${stage}/pro-monthly-price-id`),
    stripeProYearlyPriceId: isEphemeral ? "price_pro_yearly_mock" : ssmValue(`/stripe/${stage}/pro-yearly-price-id`),
    stripeUltimateMonthlyPriceId: isEphemeral
      ? "price_ultimate_monthly_mock"
      : ssmValue(`/stripe/${stage}/ultimate-monthly-price-id`),
    stripeUltimateYearlyPriceId: isEphemeral
      ? "price_ultimate_yearly_mock"
      : ssmValue(`/stripe/${stage}/ultimate-yearly-price-id`),
    localStackMappedPort: options.localStackMappedPort ?? "4566",
  };
}

export function ssmValue(path: string): string {
  return `{{resolve:ssm:${path}}}`;
}


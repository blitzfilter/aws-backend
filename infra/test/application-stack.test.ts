import * as cdk from "aws-cdk-lib";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { STAGES, type StageName } from "../src/config";

function createStacks(stage: StageName) {
  const app = new cdk.App({ analyticsReporting: false });
  return createApplicationStacks(app, {
    stage,
    stackNamePrefix: `application-${stage}`,
  });
}

describe("Application stacks", () => {
  test.each(STAGES)("synthesizes the %s stack contract", (stage) => {
    const stacks = createStacks(stage);


    expect(Object.values(Template.fromStack(stacks.compute).findResources("AWS::Cognito::UserPool"))).toHaveLength(1);
    expect(Object.values(Template.fromStack(stacks.api).findResources("AWS::ApiGatewayV2::Api"))).toHaveLength(1);
    expect(Object.values(Template.fromStack(stacks.api).findResources("AWS::ApiGatewayV2::Route"))).toHaveLength(0);
    expect(Object.values(Template.fromStack(stacks.compute).findResources("AWS::StepFunctions::StateMachine"))).toHaveLength(0);
  });

  test.each(STAGES)("passes explicit PostgreSQL TLS policy to every %s database Lambda", (stage) => {
    const stacks = createStacks(stage);
    const functions = Object.values(Template.fromStack(stacks.compute).findResources("AWS::Lambda::Function"));
    const databaseFunctions = functions.filter((resource) => resource.Properties.Environment?.Variables?.POSTGRES_HOST !== undefined);
    expect(databaseFunctions).toHaveLength(stage === "ephemeral" ? 3 : 4);
    for (const resource of databaseFunctions) {
      const environment = resource.Properties.Environment.Variables;
      expect(environment.STAGE).toBe(stage);
      expect(environment.POSTGRES_SSL_MODE).toBe(stage === "ephemeral" ? "disable" : "verify-full");
      expect(environment.POSTGRES_MAX_CONNECTIONS).toBe("2");
      if (stage === "ephemeral") {
        expect(environment.POSTGRES_SSL_ROOT_CERT).toBeUndefined();
      } else {
        expect(environment.POSTGRES_SSL_ROOT_CERT).toBe(`{{resolve:ssm:/postgres/${stage}/ssl-root-cert-path}}`);
      }
      expect(environment.PGSSLMODE).toBeUndefined();
    }
  });

  test("synthesizes the ephemeral stack without CloudFront", () => {
    const app = new cdk.App({ analyticsReporting: false });
    const template = Template.fromStack(new ApplicationEphemeralStack(app, "application-ephemeral", { stage: "ephemeral" }));

    expect(Object.values(template.findResources("AWS::OpenSearchService::Domain"))).toHaveLength(1);
    expect(Object.values(template.findResources("AWS::CloudFront::Distribution"))).toHaveLength(0);
  });
});

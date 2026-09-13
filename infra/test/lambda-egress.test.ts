import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import { Template } from "aws-cdk-lib/assertions";
import { ApplicationEphemeralStack, createApplicationStacks } from "../src/application-stack";
import { STAGES } from "../src/config";
import { LambdaEgress, type LambdaEgressProps } from "../src/constructs/lambda-egress";

const ENVIRONMENT = { account: "123456789012", region: "eu-central-1" };
const AZS = [
  { availabilityZone: "eu-central-1a", publicSubnetCidr: "10.42.0.0/24", privateSubnetCidr: "10.42.16.0/24" },
  { availabilityZone: "eu-central-1b", publicSubnetCidr: "10.42.1.0/24", privateSubnetCidr: "10.42.17.0/24" },
  { availabilityZone: "eu-central-1c", publicSubnetCidr: "10.42.2.0/24", privateSubnetCidr: "10.42.18.0/24" },
];

function props(): LambdaEgressProps {
  return {
    stage: "dev",
    environment: { ...ENVIRONMENT },
    ipProtocol: "IPV4",
    vpcCidr: "10.42.0.0/16",
    availabilityZones: AZS.map((layout) => ({ ...layout })),
    natTopology: { mode: "SINGLE", availabilityZone: "eu-central-1b" },
    // Public-address syntax fixtures only. Tests never connect to these addresses.
    database: { destinationCidr: "8.8.8.8/32", port: 5432 },
    httpsPolicy: { mode: "PUBLIC_IPV4" },
  };
}

function fixture(input = props(), environment: cdk.Environment = ENVIRONMENT) {
  const app = new cdk.App({ analyticsReporting: false });
  const stack = new cdk.Stack(app, "EgressTest", { env: environment });
  const construct = new LambdaEgress(stack, "Egress", input);
  return { app, stack, output: construct.output, template: Template.fromStack(stack) };
}

function resourceByRef(template: Template, reference: { Ref: string }) {
  return template.toJSON().Resources[reference.Ref];
}

function expectTopology(input: LambdaEgressProps) {
  const { app, stack, template, output } = fixture(input);
  const count = input.availabilityZones.length;
  const natCount = input.natTopology.mode === "SINGLE" ? 1 : count;
  template.resourceCountIs("AWS::EC2::VPC", 1);
  template.resourceCountIs("AWS::EC2::Subnet", count * 2);
  template.resourceCountIs("AWS::EC2::RouteTable", count * 2);
  template.resourceCountIs("AWS::EC2::SubnetRouteTableAssociation", count * 2);
  template.resourceCountIs("AWS::EC2::InternetGateway", 1);
  template.resourceCountIs("AWS::EC2::VPCGatewayAttachment", 1);
  template.resourceCountIs("AWS::EC2::NatGateway", natCount);
  template.resourceCountIs("AWS::EC2::EIP", natCount);
  template.resourceCountIs("AWS::EC2::Route", count * 2);
  template.resourceCountIs("AWS::EC2::SecurityGroup", 1);
  template.hasResourceProperties("AWS::EC2::VPC", {
    CidrBlock: input.vpcCidr, EnableDnsSupport: true, EnableDnsHostnames: true,
  });
  expect(output.stage).toBe(input.stage);
  expect(output.environment).toEqual(input.environment);
  expect(output.privateSubnets).toHaveLength(count);
  expect(output.publicSubnets).toHaveLength(count);
  expect(output.natGateways).toHaveLength(natCount);
  expect(output.vpc.selectSubnets({ subnetType: ec2.SubnetType.PRIVATE_WITH_EGRESS }).subnets).toEqual(output.privateSubnets);
  expect(output.vpc.selectSubnets({ subnetType: ec2.SubnetType.PUBLIC }).subnets).toEqual(output.publicSubnets);
  expect(output.vpc.selectSubnets({ subnets: [...output.privateSubnets] }).hasPublic).toBe(false);

  const routes = Object.entries(template.findResources("AWS::EC2::Route"));
  const attachment = Object.keys(template.findResources("AWS::EC2::VPCGatewayAttachment"))[0];
  const gateway = Object.keys(template.findResources("AWS::EC2::InternetGateway"))[0];
  expect(template.toJSON().Resources[attachment].Properties).toEqual({
    VpcId: stack.resolve(output.vpc.vpcId), InternetGatewayId: { Ref: gateway },
  });
  for (const [index, layout] of input.availabilityZones.entries()) {
    for (const [subnet, cidr] of [
      [output.publicSubnets[index], layout.publicSubnetCidr],
      [output.privateSubnets[index], layout.privateSubnetCidr],
    ] as const) {
      const subnetRef = stack.resolve(subnet.subnetId);
      expect(resourceByRef(template, subnetRef).Properties).toMatchObject({
        VpcId: stack.resolve(output.vpc.vpcId), CidrBlock: cidr,
        AvailabilityZone: layout.availabilityZone, MapPublicIpOnLaunch: false,
      });
      template.hasResourceProperties("AWS::EC2::SubnetRouteTableAssociation", {
        SubnetId: subnetRef, RouteTableId: stack.resolve(subnet.routeTable.routeTableId),
      });
    }
    const publicRouteTable = stack.resolve(output.publicSubnets[index].routeTable.routeTableId);
    const publicRoutes = routes.filter(([, route]) => JSON.stringify(route.Properties.RouteTableId) === JSON.stringify(publicRouteTable));
    expect(publicRoutes).toHaveLength(1);
    expect(publicRoutes[0][1].Properties).toEqual({
      RouteTableId: publicRouteTable, DestinationCidrBlock: "0.0.0.0/0", GatewayId: { Ref: gateway },
    });
    expect(publicRoutes[0][1].DependsOn).toContain(attachment);

    const topology = input.natTopology;
    const expectedNat = topology.mode === "SINGLE"
      ? output.natGateways.find((nat) => nat.availabilityZone === topology.availabilityZone)!
      : output.natGateways.find((nat) => nat.availabilityZone === layout.availabilityZone)!;
    expect(expectedNat).toBeDefined();
    const privateRouteTable = stack.resolve(output.privateSubnets[index].routeTable.routeTableId);
    const privateRoutes = routes.filter(([, route]) => JSON.stringify(route.Properties.RouteTableId) === JSON.stringify(privateRouteTable));
    expect(privateRoutes).toHaveLength(1);
    expect(privateRoutes[0][1].Properties).toEqual({
      RouteTableId: privateRouteTable, DestinationCidrBlock: "0.0.0.0/0",
      NatGatewayId: stack.resolve(expectedNat.natGatewayId),
    });
  }
  for (const nat of output.natGateways) {
    const natResource = resourceByRef(template, stack.resolve(nat.natGatewayId));
    const { Tags: _tags, ...natProperties } = natResource.Properties;
        expect(natProperties).toEqual({
      AllocationId: stack.resolve(nat.eipAllocationId),
      ConnectivityType: "public", SubnetId: stack.resolve(nat.publicSubnet.subnetId),
    });
    const publicRouteTable = stack.resolve(nat.publicSubnet.routeTable.routeTableId);
    const [publicRouteId] = routes.find(([, route]) => JSON.stringify(route.Properties.RouteTableId) === JSON.stringify(publicRouteTable))!;
    expect(natResource.DependsOn).toContain(publicRouteId);
    const eipRef = stack.resolve(nat.publicIpv4);
    expect(resourceByRef(template, eipRef).Properties.Domain).toBe("vpc");
    expect(stack.resolve(nat.eipAllocationId)).toEqual({ "Fn::GetAtt": [eipRef.Ref, "AllocationId"] });
  }
  // No IPv6 allocation/route, RDS, Lambda, IAM, lookup/custom resource, or stack export.
  const allowedTypes = new Set([
    "AWS::EC2::VPC", "AWS::EC2::Subnet", "AWS::EC2::RouteTable", "AWS::EC2::SubnetRouteTableAssociation",
    "AWS::EC2::InternetGateway", "AWS::EC2::VPCGatewayAttachment", "AWS::EC2::NatGateway", "AWS::EC2::EIP",
    "AWS::EC2::Route", "AWS::EC2::SecurityGroup",
  ]);
  for (const resource of Object.values(template.toJSON().Resources) as { Type: string; Properties: object }[]) {
    expect(allowedTypes.has(resource.Type)).toBe(true);
    expect(JSON.stringify(resource.Properties)).not.toMatch(/Ipv6|::\/0|EgressOnly/);
  }
  expect(template.toJSON().Outputs).toBeUndefined();
  expect(app.synth().manifest.missing ?? []).toEqual([]);
}

describe("Lambda egress offline foundation", () => {
  test.each(STAGES)("%s: SINGLE routes every private subnet to the explicitly chosen NAT AZ", (stage) => {
    expectTopology({ ...props(), stage });
  });
  test.each([1, 2, 3])("PER_AZ uses exact same-AZ NAT/EIP identity in %i AZs", (count) => {
    expectTopology({ ...props(), availabilityZones: AZS.slice(0, count), natTopology: { mode: "PER_AZ" } });
  });
  test("SINGLE supports one AZ without implicit extra subnets", () => {
    expectTopology({ ...props(), availabilityZones: [AZS[1]] });
  });
  test("input ordering does not replace NAT/EIP/subnet resources", () => {
    for (const natTopology of [props().natTopology, { mode: "PER_AZ" } as const]) {
      const original = fixture({ ...props(), natTopology }).template.toJSON();
      const reordered = fixture({ ...props(), natTopology, availabilityZones: [...AZS].reverse() }).template.toJSON();
      expect(reordered).toEqual(original);
    }
  });
  test.each(["us-west-2", "us-gov-west-1", "cn-north-1"])("uses explicit compatible account/region/AZs: %s", (region) => {
    const environment = { account: "234567890123", region };
    const input: LambdaEgressProps = {
      ...props(), environment, natTopology: { mode: "PER_AZ" },
      availabilityZones: [{ ...AZS[0], availabilityZone: `${region}a` }],
    };
    const { template } = fixture(input, environment);
    template.hasResourceProperties("AWS::EC2::Subnet", { AvailabilityZone: `${region}a` });
  });
  test.each([
    { mode: "PUBLIC_IPV4" } as const,
    { mode: "CIDR_ALLOWLIST", destinationCidrs: ["1.1.1.0/24", "8.8.4.4/32"] } as const,
  ])("only exact DB TCP and explicit HTTPS egress: $mode", (httpsPolicy) => {
    const input = { ...props(), database: { destinationCidr: "8.8.8.8/32", port: 6432 }, httpsPolicy };
    const { stack, template, output } = fixture(input);
    const [[groupId, group]] = Object.entries(template.findResources("AWS::EC2::SecurityGroup"));
        expect(stack.resolve(output.securityGroup.securityGroupId)).toEqual({ "Fn::GetAtt": [groupId, "GroupId"] });
    const cidrs = httpsPolicy.mode === "PUBLIC_IPV4" ? ["0.0.0.0/0"] : httpsPolicy.destinationCidrs;
    expect(group.Properties.SecurityGroupIngress ?? []).toEqual([]);
    expect(group.Properties.SecurityGroupEgress).toEqual([
      { CidrIp: "8.8.8.8/32", IpProtocol: "tcp", FromPort: 6432, ToPort: 6432, Description: "Self-hosted PostgreSQL only" },
      ...cidrs.map((CidrIp) => ({
        CidrIp, IpProtocol: "tcp", FromPort: 443, ToPort: 443, Description: "Explicit AWS/provider HTTPS policy",
      })),
    ]);
    template.resourceCountIs("AWS::EC2::SecurityGroupIngress", 0);
    template.resourceCountIs("AWS::EC2::SecurityGroupEgress", 0);
  });
  test.each(["10.0.0.0/16", "172.16.0.0/16", "192.168.0.0/16"])("accepts RFC1918 VPC %s", (vpcCidr) => {
    const prefix = vpcCidr.split(".").slice(0, 2).join(".");
    fixture({ ...props(), vpcCidr, natTopology: { mode: "PER_AZ" }, availabilityZones: [{
      availabilityZone: AZS[0].availabilityZone,
      publicSubnetCidr: `${prefix}.0.0/28`, privateSubnetCidr: `${prefix}.0.16/28`,
    }] });
  });
});

function rejected(patch: Record<string, unknown>, field: string) {
  const app = new cdk.App({ analyticsReporting: false });
  const stack = new cdk.Stack(app, "Rejected", { env: ENVIRONMENT });
  expect(() => new LambdaEgress(stack, "Egress", { ...props(), ...patch } as LambdaEgressProps))
    .toThrow(new Error(`Invalid Lambda egress ${field}.`));
  expect(stack.node.findAll().filter((node) => node instanceof cdk.CfnResource)).toHaveLength(0);
}

describe("fail-closed inputs (before any resources)", () => {
  test.each([undefined, null, "", "production", "local", "test", "DEV", "dev\n", "secret-canary"])("rejects stage %p", (stage) => {
    rejected({ stage }, "stage");
  });
  test.each(["DUAL_STACK", "IPV6", undefined, null])("rejects IP protocol %p", (ipProtocol) => {
    rejected({ ipProtocol }, "IP protocol");
  });
  test.each(["", "123", "000000000000", "123456789012\n", 123456789012])("rejects account %p", (account) => {
    rejected({ environment: { ...ENVIRONMENT, account } }, "account");
  });
  test.each(["", "EU-CENTRAL-1", "eu-central-1a", "eu-central-1\n", undefined])("rejects region %p", (region) => {
    rejected({ environment: { ...ENVIRONMENT, region } }, "region");
  });
  test.each([{}, { account: ENVIRONMENT.account }, { ...ENVIRONMENT, account: "234567890123" }, { ...ENVIRONMENT, region: "us-east-1" }])(
    "rejects agnostic or mismatched stack environment %p", (environment) => {
      expect(() => fixture(props(), environment)).toThrow("Invalid Lambda egress stack environment.");
    },
  );
  test.each([
    "0.0.0.0/0", "8.8.0.0/16", "10.0.0.0/8", "10.42.0.0/29", "10.42.0.1/16",
    "10.042.0.0/16", "10.42.0.0/016", "10.256.0.0/16", "10.42.0.0/33", "10.42.0.0/-1",
    "10.42.0.0", "10.42.0.0/16\n", " 10.42.0.0/16", "::/0", "postgres://secret-canary@host/db", undefined,
  ])("rejects unsafe/malformed VPC CIDR %p", (vpcCidr) => rejected({ vpcCidr }, "VPC CIDR"));
  test.each([[], [AZS[0], AZS[0]], [...AZS, { ...AZS[0], availabilityZone: "eu-central-1d" }], undefined, [null]]
    .map((availabilityZones) => ({ availabilityZones })))(
    "rejects missing/duplicate/oversized AZ layout %#", ({ availabilityZones }) => rejected({ availabilityZones }, "AZ layout"),
  );
  test.each(["us-east-1a", "eu-central-1", "eu-central-1aa", "euc1-az1", "eu-central-1-waw-1a", "eu-central-1a\n"])(
    "rejects incompatible/nonstandard AZ %s", (availabilityZone) => {
      rejected({ availabilityZones: [{ ...AZS[0], availabilityZone }] }, "AZ layout");
    },
  );
  test.each(["10.43.0.0/24", "10.42.0.1/24", "10.42.0.0/29", "10.42.0.0/15", "0.0.0.0/0", "::/64", undefined])(
    "rejects malformed/outside/unsafe subnet %p", (privateSubnetCidr) => {
      rejected({ availabilityZones: [{ ...AZS[0], privateSubnetCidr }] }, "subnet CIDR");
    },
  );
  test.each([
    [{ ...AZS[0], privateSubnetCidr: AZS[0].publicSubnetCidr }],
    [{ ...AZS[0], publicSubnetCidr: "10.42.0.0/20", privateSubnetCidr: "10.42.1.0/24" }],
    [AZS[0], { ...AZS[1], publicSubnetCidr: AZS[0].privateSubnetCidr }],
    [AZS[0], { ...AZS[1], privateSubnetCidr: AZS[0].privateSubnetCidr }],
  ])("rejects exact/nested/cross-AZ subnet overlap %#", (...availabilityZones) => {
    rejected({ availabilityZones }, "subnet overlap");
  });
  test.each([undefined, {}, { mode: "AUTO" }, { mode: "single" }])("requires explicit NAT topology %p", (natTopology) => {
    rejected({ natTopology }, "NAT topology");
  });
  test.each([undefined, "eu-central-1d", "us-east-1a"])("requires selected SINGLE NAT AZ %p", (availabilityZone) => {
    rejected({ natTopology: { mode: "SINGLE", availabilityZone } }, "NAT AZ");
  });
  test.each([
    "0.0.0.0/32", "10.0.0.1/32", "100.64.0.1/32", "127.0.0.1/32", "169.254.169.254/32",
    "172.16.0.1/32", "192.0.0.9/32", "192.0.2.1/32", "192.88.99.1/32", "192.168.0.1/32",
    "198.18.0.1/32", "198.51.100.1/32", "203.0.113.1/32", "224.0.0.1/32", "240.0.0.1/32",
    "255.255.255.255/32", "0.0.0.0/0", "8.8.8.0/24", "8.8.8.8", "8.8.8.8/32\n", "8.8.8.256/32",
    "::ffff:8.8.8.8/128", "db.example.com/32", "postgres://secret-canary@host/db", undefined,
  ])("rejects non-public/non-/32/malformed database destination %p", (destinationCidr) => {
    rejected({ database: { destinationCidr, port: 5432 } }, "database destination");
  });
  test.each([0, -1, 65536, 5432.5, NaN, Infinity, "5432", undefined, 443])("rejects invalid or HTTPS-bypassed DB port %p", (port) => {
    rejected({ database: { destinationCidr: "8.8.8.8/32", port } }, "database port");
  });
  test.each([undefined, null, {}, { mode: "AUTO" }, { mode: "DISABLED" }])("requires explicit HTTPS policy %p", (httpsPolicy) => {
    rejected({ httpsPolicy }, "HTTPS policy");
  });
  test.each([[], undefined, Array(51).fill("1.1.1.1/32")].map((destinationCidrs) => ({ destinationCidrs })))("rejects empty/oversized HTTPS allowlist %#", ({ destinationCidrs }) => {
    rejected({ httpsPolicy: { mode: "CIDR_ALLOWLIST", destinationCidrs } }, "HTTPS destinations");
  });
  test.each(["0.0.0.0/0", "8.0.0.0/6", "10.0.0.0/8", "169.254.169.254/32", "203.0.113.0/24", "::/0", "https://secret-canary"])(
    "rejects unsafe HTTPS destination %p", (cidr) => {
      rejected({ httpsPolicy: { mode: "CIDR_ALLOWLIST", destinationCidrs: [cidr] } }, "HTTPS destination");
    },
  );
  test.each([["1.1.1.0/24", "1.1.1.1/32"], ["1.1.1.1/32", "1.1.1.1/32"]])("rejects HTTPS overlap %#", (...destinationCidrs) => {
    rejected({ httpsPolicy: { mode: "CIDR_ALLOWLIST", destinationCidrs } }, "HTTPS overlap");
  });
  test("rejects unresolved tokens and dynamic references without resolving or echoing them", () => {
    const token = cdk.Lazy.string({ produce: () => { throw new Error("secret-canary must not resolve"); } });
    for (const value of [token, "{{resolve:ssm:/secret-canary}}", "{{resolve:secretsmanager:secret-canary}}"]) {
      rejected({ stage: value }, "stage");
      rejected({ environment: { ...ENVIRONMENT, account: value } }, "account");
      rejected({ environment: { ...ENVIRONMENT, region: value } }, "region");
      rejected({ vpcCidr: value }, "VPC CIDR");
      rejected({ availabilityZones: [{ ...AZS[0], availabilityZone: value }] }, "AZ layout");
      rejected({ availabilityZones: [{ ...AZS[0], publicSubnetCidr: value }] }, "subnet CIDR");
      rejected({ natTopology: { mode: "SINGLE", availabilityZone: value } }, "NAT AZ");
      rejected({ natTopology: { mode: value } }, "NAT topology");
      rejected({ database: { destinationCidr: value, port: 5432 } }, "database destination");
      rejected({ httpsPolicy: { mode: "CIDR_ALLOWLIST", destinationCidrs: [value] } }, "HTTPS destination");
    }
    const number = cdk.Lazy.number({ produce: () => { throw new Error("secret-canary must not resolve"); } });
    rejected({ database: { destinationCidr: "8.8.8.8/32", port: number } }, "database port");
    const list = cdk.Lazy.list({ produce: () => { throw new Error("secret-canary must not resolve"); } });
    rejected({ httpsPolicy: { mode: "CIDR_ALLOWLIST", destinationCidrs: list } }, "HTTPS destinations");
    rejected({ availabilityZones: list }, "AZ layout");
  });
  test.each([undefined, null])("rejects absent structured configuration %p", (value) => {
    rejected({ environment: value }, "environment");
    rejected({ database: value }, "database policy");
    const stack = new cdk.Stack(new cdk.App({ analyticsReporting: false }), "Invalid", { env: ENVIRONMENT });
    expect(() => new LambdaEgress(stack, "Egress", value as unknown as LambdaEgressProps))
      .toThrow("Invalid Lambda egress configuration.");
  });
});

function expectApplicationUnattached(stack: cdk.Stack) {
  const template = Template.fromStack(stack);
  for (const type of ["AWS::EC2::VPC", "AWS::EC2::NatGateway", "AWS::EC2::EIP", "AWS::EC2::SecurityGroup", "AWS::RDS::DBInstance"]) {
    template.resourceCountIs(type, 0);
  }
  for (const lambda of Object.values(template.findResources("AWS::Lambda::Function"))) {
    expect(lambda.Properties.VpcConfig).toBeUndefined();
  }
}

test.each(STAGES)("current %s application remains unattached", (stage) => {
  const app = new cdk.App({ analyticsReporting: false });
  const stacks = createApplicationStacks(app, { stage, stackNamePrefix: `application-${stage}` });
  for (const stack of Object.values(stacks)) {
      if (stack) expectApplicationUnattached(stack);
    }
});

test("current single-stack ephemeral application remains unattached", () => {
  const app = new cdk.App({ analyticsReporting: false });
  expectApplicationUnattached(new ApplicationEphemeralStack(app, "application-ephemeral", { stage: "ephemeral" }));
});

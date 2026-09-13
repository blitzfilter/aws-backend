import * as cdk from "aws-cdk-lib";
import * as ec2 from "aws-cdk-lib/aws-ec2";
import { Construct } from "constructs";
import { isStageName, type StageName } from "../config";

export interface LambdaEgressAzLayout {
  /** Standard AZ name in the explicit stack account/region; no lookup is performed. */
  readonly availabilityZone: string;
  readonly publicSubnetCidr: string;
  readonly privateSubnetCidr: string;
}

export type LambdaEgressNatTopology =
  // Lower fixed cost; cross-AZ charges and loss of this AZ affect all private subnets.
  | { readonly mode: "SINGLE"; readonly availabilityZone: string }
  // One billed NAT/EIP per AZ; private routes never depend on another AZ's NAT.
  | { readonly mode: "PER_AZ" };

export type LambdaEgressHttpsPolicy =
  // Explicitly permits any IPv4 destination on TCP 443, not just AWS/provider hosts.
  | { readonly mode: "PUBLIC_IPV4" }
  | { readonly mode: "CIDR_ALLOWLIST"; readonly destinationCidrs: readonly string[] };

export interface LambdaEgressProps {
  readonly stage: StageName;
  readonly environment: { readonly account: string; readonly region: string };
  readonly ipProtocol: "IPV4";
  /** Canonical RFC1918 network, /16 through /28. */
  readonly vpcCidr: string;
  /** One public NAT subnet and one private Lambda subnet per AZ, 1–3 AZs. */
  readonly availabilityZones: readonly LambdaEgressAzLayout[];
  readonly natTopology: LambdaEgressNatTopology;
  readonly database: { readonly destinationCidr: string; readonly port: number };
  readonly httpsPolicy: LambdaEgressHttpsPolicy;
}

export interface LambdaEgressNatOutput {
  readonly availabilityZone: string;
  readonly publicSubnet: ec2.ISubnet;
  readonly natGatewayId: string;
  /** CloudFormation values, available only after a future authorized deployment. */
  readonly eipAllocationId: string;
  readonly publicIpv4: string;
}

export interface LambdaEgressOutput {
  readonly stage: StageName;
  readonly environment: { readonly account: string; readonly region: string };
  readonly vpc: ec2.IVpc;
  readonly publicSubnets: readonly ec2.ISubnet[];
  readonly privateSubnets: readonly ec2.ISubnet[];
  readonly securityGroup: ec2.ISecurityGroup;
  readonly natGateways: readonly LambdaEgressNatOutput[];
}

/** Offline network foundation only. Does not attach Lambdas, export stacks, or configure PG. */
export class LambdaEgress extends Construct {
  readonly output: LambdaEgressOutput;

  constructor(scope: Construct, id: string, props: LambdaEgressProps) {
    super(scope, id);
    validateProps(cdk.Stack.of(this), props);

    // Pinned L2 Vpc consults stack.availabilityZones even with explicit AZs, requesting
    // a context lookup. L1 avoids that and leaves IPv6 unallocated. No default-SG custom resource.
    const vpcResource = new ec2.CfnVPC(this, "Vpc", {
      cidrBlock: props.vpcCidr,
      enableDnsSupport: true,
      enableDnsHostnames: true,
      instanceTenancy: "default",
      tags: [{ key: "Name", value: `aura-lambda-egress-${props.stage}` }],
    });
    // Typed handle to the resource above, not an existing-VPC lookup. Register the actual
    // L2 subnets below so selection preserves their Internet-route dependencies.
    const vpc = ec2.Vpc.fromVpcAttributes(this, "VpcHandle", {
      vpcId: vpcResource.ref,
      vpcCidrBlock: props.vpcCidr,
      availabilityZones: props.availabilityZones.map((layout) => layout.availabilityZone),
    });
    // L1 gateway/EIP glue accompanies L2 subnets with operator-supplied exact CIDRs.
    const gateway = new ec2.CfnInternetGateway(this, "InternetGateway");
    const attachment = new ec2.CfnVPCGatewayAttachment(this, "InternetGatewayAttachment", {
      vpcId: vpc.vpcId,
      internetGatewayId: gateway.ref,
    });
    const subnets = props.availabilityZones.map((layout) => {
      // AZ-keyed IDs keep layout reordering from replacing subnets or EIPs.
      const azScope = new Construct(this, layout.availabilityZone);
      const publicSubnet = new ec2.PublicSubnet(azScope, "Public", {
        vpcId: vpc.vpcId,
        availabilityZone: layout.availabilityZone,
        cidrBlock: layout.publicSubnetCidr,
        mapPublicIpOnLaunch: false,
      });
      publicSubnet.addDefaultInternetRoute(gateway.ref, attachment);
      const privateSubnet = new ec2.PrivateSubnet(azScope, "Private", {
        vpcId: vpc.vpcId,
        availabilityZone: layout.availabilityZone,
        cidrBlock: layout.privateSubnetCidr,
        mapPublicIpOnLaunch: false,
      });
      vpc.publicSubnets.push(publicSubnet);
      vpc.privateSubnets.push(privateSubnet);
      return { azScope, publicSubnet, privateSubnet };
    });
    const topology = props.natTopology;
    const natSubnets = topology.mode === "SINGLE"
      ? subnets.filter(({ publicSubnet }) => publicSubnet.availabilityZone === topology.availabilityZone)
      : subnets;
    const natGateways = natSubnets.map(({ azScope, publicSubnet }): LambdaEgressNatOutput => {
      const eip = new ec2.CfnEIP(azScope, "NatEip", { domain: "vpc" });
      const nat = publicSubnet.addNatGateway(eip.attrAllocationId);
      nat.connectivityType = "public";
      return {
        availabilityZone: publicSubnet.availabilityZone,
        publicSubnet,
        natGatewayId: nat.ref,
        eipAllocationId: eip.attrAllocationId,
        publicIpv4: eip.ref,
      };
    });
    for (const { privateSubnet } of subnets) {
      const nat = topology.mode === "SINGLE"
        ? natGateways[0]
        : natGateways.find((candidate) => candidate.availabilityZone === privateSubnet.availabilityZone)!;
      privateSubnet.addDefaultNatRoute(nat.natGatewayId);
    }

    const securityGroup = new ec2.SecurityGroup(this, "LambdaSecurityGroup", {
      vpc,
      description: `Dedicated IPv4 Lambda egress (${props.stage}); no ingress`,
      allowAllOutbound: false,
      allowAllIpv6Outbound: false,
      disableInlineRules: false,
    });
    securityGroup.addEgressRule(
      ec2.Peer.ipv4(props.database.destinationCidr),
      ec2.Port.tcp(props.database.port),
      "Self-hosted PostgreSQL only",
    );
    const httpsCidrs = props.httpsPolicy.mode === "PUBLIC_IPV4"
      ? ["0.0.0.0/0"]
      : props.httpsPolicy.destinationCidrs;
    for (const cidr of httpsCidrs) {
      securityGroup.addEgressRule(ec2.Peer.ipv4(cidr), ec2.Port.tcp(443), "Explicit AWS/provider HTTPS policy");
    }
    cdk.Tags.of(this).add("Stage", props.stage);
    this.output = {
      stage: props.stage,
      environment: { ...props.environment },
      vpc,
      publicSubnets: subnets.map(({ publicSubnet }) => publicSubnet),
      privateSubnets: subnets.map(({ privateSubnet }) => privateSubnet),
      securityGroup,
      natGateways,
    };
  }
}

interface Ipv4Range {
  readonly start: number;
  readonly end: number;
  readonly prefix: number;
}

function invalid(field: string): never {
  // Only developer-owned field names; never interpolate supplied values or chain parser errors.
  throw new Error(`Invalid Lambda egress ${field}.`);
}

function literal(value: unknown): value is string {
  return typeof value === "string" && value === value.trim() && !cdk.Token.isUnresolved(value);
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value) && !cdk.Token.isUnresolved(value);
}

function parseCidr(value: unknown, field: string): Ipv4Range {
  if (!literal(value) || !/^(?:0|[1-9]\d{0,2})(?:\.(?:0|[1-9]\d{0,2})){3}\/(?:0|[1-9]\d?)$/.test(value)) {
    invalid(field);
  }
  const [address, mask] = value.split("/");
  const octets = address.split(".").map(Number);
  const prefix = Number(mask);
  if (octets.some((octet) => octet > 255) || prefix > 32) invalid(field);
  const start = octets.reduce((number, octet) => number * 256 + octet, 0);
  const size = 2 ** (32 - prefix);
  if (start % size !== 0) invalid(field);
  return { start, end: start + size - 1, prefix };
}

function contains(outer: Ipv4Range, inner: Ipv4Range): boolean {
  return outer.start <= inner.start && inner.end <= outer.end;
}

function overlaps(left: Ipv4Range, right: Ipv4Range): boolean {
  return left.start <= right.end && right.start <= left.end;
}

const PRIVATE_RANGES = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"]
  .map((cidr) => parseCidr(cidr, "internal CIDR"));
// Conservative exclusions: private, shared, loopback, link-local/metadata, documentation,
// benchmark, special-purpose, multicast and reserved space. No live IP registry lookup.
const NON_PUBLIC_RANGES = [
  "0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16",
  "172.16.0.0/12", "192.0.0.0/24", "192.0.2.0/24", "192.88.99.0/24", "192.168.0.0/16",
  "198.18.0.0/15", "198.51.100.0/24", "203.0.113.0/24", "224.0.0.0/4", "240.0.0.0/4",
].map((cidr) => parseCidr(cidr, "internal CIDR"));

function publicCidr(value: unknown, field: string): Ipv4Range {
  const range = parseCidr(value, field);
  if (NON_PUBLIC_RANGES.some((excluded) => overlaps(excluded, range))) invalid(field);
  return range;
}

function validateProps(stack: cdk.Stack, props: LambdaEgressProps): void {
  if (!record(props)) invalid("configuration");
  if (!literal(props.stage) || !isStageName(props.stage)) invalid("stage");
  if (!record(props.environment)) invalid("environment");
  const { account, region } = props.environment;
  if (!literal(account) || !/^\d{12}$/.test(account) || account === "000000000000") invalid("account");
  if (!literal(region) || !/^[a-z]{2}(?:-[a-z]+)+-[1-9]\d*$/.test(region)) invalid("region");
  if (!literal(stack.account) || !literal(stack.region) || stack.account !== account || stack.region !== region) {
    invalid("stack environment");
  }
  if (props.ipProtocol !== "IPV4") invalid("IP protocol");
  const vpc = parseCidr(props.vpcCidr, "VPC CIDR");
  if (vpc.prefix < 16 || vpc.prefix > 28 || !PRIVATE_RANGES.some((range) => contains(range, vpc))) {
    invalid("VPC CIDR");
  }
  if (!Array.isArray(props.availabilityZones) || cdk.Token.isUnresolved(props.availabilityZones)
    || props.availabilityZones.length < 1 || props.availabilityZones.length > 3) invalid("AZ layout");
  const zones = new Set<string>();
  const subnetRanges: Ipv4Range[] = [];
  for (const layout of props.availabilityZones) {
    if (!record(layout) || !literal(layout.availabilityZone)
      || !layout.availabilityZone.startsWith(region)
      || !/^[a-z]$/.test(layout.availabilityZone.slice(region.length))
      || zones.has(layout.availabilityZone)) invalid("AZ layout");
    zones.add(layout.availabilityZone);
    for (const cidr of [layout.publicSubnetCidr, layout.privateSubnetCidr]) {
      const range = parseCidr(cidr, "subnet CIDR");
      if (range.prefix < 16 || range.prefix > 28 || !contains(vpc, range)) invalid("subnet CIDR");
      if (subnetRanges.some((previous) => overlaps(previous, range))) invalid("subnet overlap");
      subnetRanges.push(range);
    }
  }
  if (!record(props.natTopology)) invalid("NAT topology");
  if (props.natTopology.mode === "SINGLE") {
    if (!literal(props.natTopology.availabilityZone) || !zones.has(props.natTopology.availabilityZone)) {
      invalid("NAT AZ");
    }
  } else if (props.natTopology.mode !== "PER_AZ") invalid("NAT topology");
  if (!record(props.database)) invalid("database policy");
  if (publicCidr(props.database.destinationCidr, "database destination").prefix !== 32) invalid("database destination");
  // Port 443 would let the HTTPS rule bypass the database /32 boundary.
  if (!Number.isInteger(props.database.port) || cdk.Token.isUnresolved(props.database.port)
    || props.database.port < 1 || props.database.port > 65535 || props.database.port === 443) invalid("database port");
  if (!record(props.httpsPolicy)) invalid("HTTPS policy");
  if (props.httpsPolicy.mode === "CIDR_ALLOWLIST") {
    const cidrs = props.httpsPolicy.destinationCidrs;
    if (!Array.isArray(cidrs) || cdk.Token.isUnresolved(cidrs) || cidrs.length < 1 || cidrs.length > 50) {
      invalid("HTTPS destinations");
    }
    const ranges: Ipv4Range[] = [];
    for (const cidr of cidrs) {
      const range = publicCidr(cidr, "HTTPS destination");
      if (ranges.some((previous) => overlaps(previous, range))) invalid("HTTPS overlap");
      ranges.push(range);
    }
  } else if (props.httpsPolicy.mode !== "PUBLIC_IPV4") invalid("HTTPS policy");
}

# 09a — offline Lambda-egress foundation

Baseline: `b89d8942194f1ca676b487a9a89febb6f502f462`.
This slice is **not complete iteration 09**, deployment readiness, or network acceptance.
No application instantiates the construct. No cloud/host changes or secrets needed.
Shared README/AGENTS and rollout status index this contract; no deployment entrypoint is added.

## Files and public contract

- `src/constructs/lambda-egress.ts`: `LambdaEgress`, `LambdaEgressProps`,
  `LambdaEgressAzLayout`, `LambdaEgressNatTopology`, `LambdaEgressHttpsPolicy`,
  `LambdaEgressOutput`, `LambdaEgressNatOutput`.
- `test/lambda-egress.test.ts`: offline synth/input tests and current-application non-attachment checks.
- `LambdaEgress.output`: typed VPC, public/private subnet handles, dedicated SG,
  stage/account/region, and NAT records containing AZ, public subnet, NAT ID,
  EIP allocation ID and public IPv4. EIP values are CloudFormation tokens, not allocated addresses.
  No automatic CloudFormation outputs/exports or cross-stack wiring.

Pinned CDK `2.268.0` L2 `Vpc` requests AZ context even with explicit AZs.
Use L1 VPC plus an attributes-only L2 handle to **that newly declared VPC**;
no lookup/import of existing infrastructure. L2 public/private subnets and SG remain.
L1 IGW/attachment/EIP and L2 subnet NAT/route helpers provide exact network configuration.
Actual subnet handles are registered on the VPC handle, preserving subnet-selection route dependencies.
No architecture rule changed.

## Required operator inputs

| Input | Contract |
|---|---|
| `stage` | Explicit `dev`, `prod`, or `ephemeral`; same resource shape, no automatic local emulation |
| `environment` | Literal nonzero 12-digit AWS account and region; enclosing stack must match both |
| `ipProtocol` | Exactly `IPV4` |
| `vpcCidr` | Canonical RFC1918 IPv4 network, /16–/28 |
| `availabilityZones` | 1–3 unique standard AZ names in that region, each with exact public/private /16–/28 CIDRs, contained in VPC and pairwise disjoint |
| `natTopology` | `SINGLE` plus explicit selected AZ, or `PER_AZ` |
| `database` | One canonical public IPv4 `/32` and integer TCP port 1–65535 except 443 |
| `httpsPolicy` | Explicit `PUBLIC_IPV4`, or `CIDR_ALLOWLIST` with 1–50 disjoint public IPv4 CIDRs |

No tokens, dynamic references, URLs, DNS names, noncanonical/unsafe CIDRs, implicit stage,
implicit NAT placement or implicit HTTPS policy. Errors contain fixed field labels, not values/causes.
Public CIDRs conservatively exclude private/shared/loopback/link-local, documentation,
benchmark, special-purpose, multicast and reserved blocks. Test public addresses are syntax fixtures only.
Offline validation checks region/AZ syntax and consistency, **not** actual account AZ access,
regional service availability, public address ownership/routability, or conflicts with other networks.

## Network and cost policy

- Each public subnet routes IPv4 default traffic to the attached IGW. All subnets disable automatic public IP assignment.
- Each private subnet has only its NAT IPv4 default route, never a direct IGW route.
- `SINGLE`: one NAT/EIP, lower fixed cost; cross-AZ transfer charges and one NAT AZ failure can affect all private subnets. No automatic failover.
- `PER_AZ`: one NAT/EIP per AZ; private routes use only the same-AZ NAT. Higher NAT/public-IPv4 fixed cost. No cross-AZ NAT failover.
- NAT processing/data-transfer and public IPv4 charges apply after a future deployment; no prices guessed.
- Dedicated SG: no ingress, no unrestricted outbound default, DB `/32` TCP port plus explicit TCP 443 destinations. Port 443 is forbidden for DB because HTTPS could bypass its `/32` restriction.
- `PUBLIC_IPV4` deliberately allows TCP 443 to **any IPv4 destination**, not only AWS/providers. CIDR allowlists require operator maintenance; SGs cannot enforce hostname, application protocol or TLS identity. Required AWS/provider endpoints must remain reachable.
- No IPv6 allocation, IPv6 route, NAT64, egress-only IGW or IPv6 SG permission. No public PG ingress, RDS, Lambda, IAM, endpoint, secret or custom-resource provisioning.
- NAT is not an egress firewall. Later Lambdas must use the returned private subnets and **only** the dedicated SG. Additional/default SGs can broaden permissions. AWS's unused default SG/NACL are not hardened here; hardening via SDK custom resources is outside this slice.
- VPC DNS is enabled. AWS resolver traffic has SG filtering exceptions; this is not DNS-exfiltration prevention.
- AZ-keyed logical IDs survive layout reordering. Stack/construct/AZ renaming, removal or replacement can release/change EIPs. No retention or allowlist lifecycle automation.

## Evidence and remaining gates

Ran with existing dependencies; no install or lock/config edits. Local Node `24.19.0`, npm `11.17.0`:

- `npm --prefix infra run build` — pass.
- `npm --prefix infra test -- --runTestsByPath test/lambda-egress.test.ts --silent` — 146 pass.
- `npm --prefix infra test -- --silent` — 201 pass, 4 suites.
- Synth assertions prove exact NAT/EIP/route identities, subnet properties/associations,
  constrained SG, no IPv6/extra resources, zero missing lookup context, input rejection,
  ordering stability, all-stage application non-attachment and single-stack ephemeral non-attachment.

Initial test failures exposed test-assumption errors and the L2 VPC lookup; fixed and rerun.
Existing ts-jest TS151002 warnings remain. Unsilenced synth also warns W3010 about deliberate explicit AZs.
Integrator reran on cached pinned Node26.8.2: fresh `tsc --noEmit`, full Jest **201 pass**, and all three CLI synth stages **pass**. Synth used `--no-lookups`, disabled AWS shared-config/credential files and metadata access, explicit installed ts-node app, and suppressed template stdout. Existing deprecated-CDK/Node20-provider/unversioned-artifact warnings remain. Independent reviewer `f93c35bb-70ed-4d25-96e1-36f4d6035e98` accepted; separately146 tests,76 negative/2 valid-limit probes and both synthetic-consumer dependency paths pass. Eleven current application templates/410 resources unchanged by importing the unused construct. These are offline structural results, not network proof.
No live AWS/PG/provider tests, SDK calls, bootstrap, deployment, firewall or host changes.

Before later attachment: owner verifies account/AZ availability, CIDR capacity/conflicts,
NAT/EIP quotas and costs; resolves actual deployed EIPs for narrow external PG firewall rules;
proves outbound source IP, denied destinations and AZ failure behavior; supplies PG DNS/SAN,
CA delivery, verified TLS, rotation/overlap and connection budgets. Lambda ENI IAM,
separated identities, cross-stack integration, runtime/provider reachability and actual rollback
rehearsal remain later gates. Network synth does not prove any of these.

## Rollback

While uninstantiated: remove this fragment, the construct and its test file. No infrastructure or data rollback.
After any future attachment/deployment: code removal alone is unsafe; coordinate Lambda detachment,
NAT/EIP lifetime and external firewall allowlists before deleting network resources.

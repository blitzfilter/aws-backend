# DOX

## Purpose

- Own public docs, changelog, OpenAPI, and static site files.

## Core Design

- `swagger.yaml` be public REST contract.
- `CHANGELOG.md` tell API change by pull request.
- `storage.md` owns storage migration and repository conventions.
- `durable-worker-runbook.md` owns SQS runtime limits, external handoff, cutover, DLQ recovery, and projection-fence rollout. Runtime code and CDK remain the source for effective values; docs do not prove deployment.
- Child doc can own deeper subsystem docs when folder become durable boundary.

## Ownership

- This doc rule `docs/**`.
- Keep public docs aligned with shipped behavior. No wish-doc.
- Use `ListingSource`, `Partnership`, and `PartnershipApplication` for active contracts. Legacy names belong only in dated `CHANGELOG.md` entries.

## Local Contracts

- Read root, then here, before edit.
- If endpoint, payload, auth, error, or behavior change, update `docs/swagger.yaml` and `docs/CHANGELOG.md`.
- Update design docs when durable event flow, storage contract, or operator workflow change.
- Keep child doc index fresh.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Kill stale docs fast.
- Public contract first. Example after.

## Verification

- Diff API docs against code paths you changed.
- Check changed YAML or markdown render for obvious break.

## Child DOX Index

- `admin-overview.md` — administrator overview source and count semantics.
- `auction.md` — Auction domain contract and current implementation status.
- `auction-implementation.md` — isolated Auction iteration ownership and reset plan.
- `auction-iterations/` — per-iteration Auction handoffs and verification records.
- `party-and-listing-source.md` — Party, ListingSource, and Partnership contract.
- `product-listing.md` — canonical ProductListing domain contract.
- `product-listing-raw-normalization-runbook.md` — raw capture, durable wake-ups, reconstructible reconciliation cursor/FIFO, shutdown, backlog, and crawler checks.
- `durable-worker-runbook.md` — durable SQS settings, identity/Sequin handoff, safe operations, tombstones, and notification recovery.
- `product-listing-inventory.md` — ProductListing rewrite scope and final scan checklist.
- `listing-source-partnership-rewrite-inventory.md` — ListingSource and Partnership rewrite completion checklist.
- `object-ids.md` — prefixed UUIDv7 object-ID registry and boundary contract.
- `storage.md` — canonical storage contracts.
- `events/flow.md` — durable event and scheduled-flow contracts.
- `swagger.yaml` — public REST contract

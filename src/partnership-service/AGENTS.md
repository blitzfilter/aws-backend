# DOX

## Purpose

- Own Partnership application and source-grant use cases.

## Core Design

- Submit stores intent only; it creates no Party or ListingSource.
- Approval owns one PostgreSQL UoW: lock the application for idempotent replay, resolve/create Party and ListingSource, find-or-create Partnership, grant membership and source access, update application, create the applicant notification through `NotificationCreatorFactory`, then commit.
- Application reads are reader-owned. Source authorization requires membership plus a ListingSource grant through the same Partnership.
- Admin application collection search authorizes in the service layer and returns a bounded cursor result with exact filters and deterministic creation/update ordering. Application views preserve applicant, state, proposal, and approval-result references for admin detail reads.
- Admin partnership collection search authorizes in the service layer and returns `AdminPartnershipSummary` values with Party summary and membership/source-grant counts. It filters by Party, member user, and ListingSource and uses a bounded deterministic `created DESC, partnership_id DESC` cursor.
- Admin Partnership detail authorizes in the service layer and returns the Partnership/Party identity, bounded current member user and ListingSource references, complete association counts, and timestamps through a transaction-bound purpose-specific reader.
- Admin Partnership membership grants and revocations authorize the actor, verify the Partnership and target User in one transaction, commit explicitly, and report changed versus no-op outcomes for structured audit logs. Revocation treats a missing membership as a successful no-op.
- Admin ListingSource grants authorize the actor, verify the Partnership and ListingSource in one transaction, require their Party IDs to match, commit explicitly, and report changed versus no-op outcomes for structured audit logs. Grant revocation uses an explicit remove capability, verifies both records, removes only the target join row, and treats an absent grant as a committed no-op without requiring Party matching.
- Each outbound capability owns one `ports/<capability>.rs` file; `ports/mod.rs` only assembles exports.
- Rejection resolves its immutable Party/ListingSource snapshot, updates application, and creates its applicant notification in the same PostgreSQL UoW. Legacy Shop partner flow is not a dependency and remains isolated.

## Verification

- `cargo check -p partnership-service`
- `cargo test -p partnership-service --all-features`

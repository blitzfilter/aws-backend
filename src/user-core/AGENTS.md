# DOX

## Purpose

- Own `user-core` crate.
- Own canonical User domain types.

## Core Design

- Domain-only crate.
- Root modules: `access_token`, `first_name`, `last_name`, `measurement_unit`, `name`, `newsletter_subscription`, `role`, `sort_user_field`, `tier`, `user`, `user_search`.
- `user::User` is canonical aggregate. Fields private. Rehydrate boundary public for adapter crates. It owns durable orthogonal `suspended` state: new users start active; rehydration supplies stored state; `suspend()` is idempotent and never changes role or tier.
- Access-token aggregate lives here; credential-core owns the canonical scope vocabulary and OAuth client ID.
- Access-token aggregate has no storage metadata; repositories/read models own timestamps.
- `newsletter_subscription::NewsletterSubscription` owns newsletter recipient values and optional linked user identity.
- User sort defaults to `Name`; no score sort in canonical user.
- Owns pure `MeasurementUnit`; adapter and API value mapping stays outside this crate.
- Uses `credential-core` for credential vocabulary, `domain-primitives` for neutral primitives, plus pure `money` and `localization` values.
- No dependency on `user-service` or adapters.

## Ownership

- This doc rule `src/user-core/**`.
- Parent doc: `src/AGENTS.md`.
- No child doc below.

## Local Contracts

- Read `AGENTS.md`, `src/AGENTS.md`, then here, before edit.
- New doc only for child crate. No module doc.
- Update this file when crate contract or dependency edge changes.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Keep business rules here.
- No persistence, transport, or runtime glue. API token extraction stays out of this core.

## Verification

- `cargo check -p user-core`
- `cargo test -p user-core --all-features`

## Child DOX Index

- None.

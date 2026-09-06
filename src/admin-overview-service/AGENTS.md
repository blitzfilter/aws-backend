# DOX

## Purpose

- Own admin overview use case and bounded count model.

## Core Design

- `GetAdminOverview` authorizes service/system or persisted User ADMIN inside one caller-owned UoW.
- One purpose-specific reader port returns the application-owned overview model.
- Anonymous actors fail as `AuthenticatedActorRequired`; non-admin users fail as forbidden.
- No core crate. No SQLx or adapter imports.

## Verification

- `cargo check -p admin-overview-service`
- `cargo test -p admin-overview-service --all-features`

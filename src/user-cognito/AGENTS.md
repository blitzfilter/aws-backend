# DOX

## Purpose

- Own Cognito User session adapter.
- Implement `user-service` session-revocation port.

## Core Design

- Resolve Aura `UserId` from Cognito `sub`; use returned Cognito username for global sign-out.
- Keep AWS SDK types and errors private.
- Never log tokens or raw provider payloads.

## Verification

- `cargo test -p user-cognito --all-features`

## Child DOX Index

- None.

# DOX

## Purpose

- Own Cognito User session adapter.
- Implement `user-service` session-revocation port.

## Core Design

- Receive the persisted opaque Cognito subject from `user-service`; use it to find the Cognito username for global sign-out. Never derive provider identity from Aura `UserId`.
- Keep AWS SDK types and errors private.
- Never log tokens or raw provider payloads.

## Verification

- `cargo test -p user-cognito --all-features`

## Child DOX Index

- None.

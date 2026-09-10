# DOX

## Purpose

- Own Cognito User session adapter.
- Implement `user-service` session-revocation port.

## Core Design

- Receive the complete persisted Cognito `(issuer, subject)` identity from `user-service`; reject issuer mismatch against the configured pool before using the subject to find the Cognito username for global sign-out. Never derive provider identity from Aura `UserId`.
- Keep AWS SDK types and errors private.
- Never log tokens or raw provider payloads.

## Verification

- `cargo test -p user-cognito --all-features`

## Child DOX Index

- None.

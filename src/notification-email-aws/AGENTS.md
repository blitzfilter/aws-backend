# DOX

## Purpose

- Own `notification-email-aws` crate.
- Send EMAIL notification deliveries with S3 templates and SES v2.

## Core Design

- Implements service-owned `NotificationChannelSender` for EMAIL only.
- Consumes the EMAIL target-reader contract from `notification-email`. Channel-specific runtime wiring resolves the current `PRIMARY` target after generic delivery claim.
- Owns email templates, localized subject/availability text, S3 keys, SES mapping, and safe provider-error classification. Availability template data uses nullable `old_availability`/`new_availability` values. Template data consumes the service-owned already-presented image URL; it does not decide listing content visibility. Watchlist price data remains in its immutable source currency; no FX conversion is applied. Missing templates and invalid/configuration failures are permanent. Transient S3/target-read failures occur before sending and remain retryable; SES acceptance uncertainty follows the safety contract below.
- Partnership application approval/rejection templates use immutable `party_name`, `listing_source_name`, and optional `image_url` snapshot fields. Takes clients and typed config through constructor. No env read. No logs.
- Adapter-local template contract test strictly renders all 15 localized ProductListing MJML sources: five SearchFilter matches, five Watchlist availability changes, and five Watchlist price changes. It verifies the delivery data fields used by each template; deploy CI compiles MJML to S3 HTML.

## Delivery Safety (#1558)

- Sender clones injected SES config with SDK retries disabled (`max_attempts = 1`); never resend inside one service attempt. S3 read retry policy stays unchanged. No detached send work.
- Known SES throttling/limit rejection or HTTP 429 → `Retryable`, code `SES_THROTTLED`. Modeled permanent rejection or parsed non-timeout 4xx → `Permanent`, code `SES_REQUEST_OR_CONFIGURATION_INVALID`. SDK construction failure is unsent/permanent, code `SES_REQUEST_BUILD_FAILED`.
- SES timeout, dispatch/transport loss, unparseable/unknown response, HTTP 408, or 5xx → `Ambiguous`, code `SES_SEND_AMBIGUOUS`. A successful response with missing/empty/whitespace receipt → `Ambiguous`, code `SES_MESSAGE_ID_MISSING`. Keep original usable provider receipt unchanged.
- Service returns ambiguity without finalizing/releasing PROCESSING or sending again. Queue remains unacknowledged; later lease reclaim may duplicate an externally accepted email. No exactly-once guarantee.
- Keep original SDK causes as boxed sources; error Display uses only static safe codes. No recipient, payload, credential, receipt, or raw-source logging.
- Unit tests cover modeled/unmodeled failures, timeout/transport/parse ambiguity, missing receipt, retained safe sources, and the enforced single-attempt SDK config. Service tests prove ambiguity keeps the lease and does not resend.

## Ownership

- This doc rules `src/notification-email-aws/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p notification-email-aws`
- `cargo test -p notification-email-aws --all-features`

# DOX

## Purpose

- Own shared concrete PostgreSQL and SQLx mechanics.

## Core Design

- `config.rs` owns validated TLS/pool inputs, SQLx options, and redacted connect errors. `lib.rs` keeps SQLx transactions. `schema.rs` owns the shared read-only business startup gate.
- Depends on `application` transaction contracts, SQLx 0.9.0, Tokio deadlines, rustls certificate validation, URL decoding, and Unix file flags.
- No entity repository, row, mapping, direct environment read, or business port.
- Composition root passes lookup closure. Production lookup must expose actual PG* settings too. No global environment mutation.

## Configuration Contract

- `PostgresTlsConfig::new(stage, mode, root_pem, application_name)` validates explicit policy. PEM bytes support already-loaded operator configuration; lookup accepts file paths only.
- `PostgresTlsConfig::from_lookup(app, get)` requires `STAGE` and `POSTGRES_SSL_MODE`, even locally.
- Stages: exactly `dev`, `prod`, `local`, `ephemeral`, `test`. Missing/unknown stage never means local.
- Modes: exactly SQLx-standard `verify-full` or `disable`. `dev/prod` require `verify-full`. Only explicit local/ephemeral/test permit `disable`.
- Every `verify-full` configuration requires explicit, nonempty, valid CA certificate PEM. `disable` rejects supplied CA input.
- `POSTGRES_SSL_ROOT_CERT` names CA file, not inline PEM. Maximum 1 MiB. Certificate bundles allowed; malformed entries, other PEM types, and surrounding garbage rejected.
- `app` is a fixed nonsecret label: 1-63 ASCII letters/digits/`.`/`_`/`-`, beginning with letter/digit. No app-name environment override.
- `PostgresPoolConfig::new(host, port, database, username, password, max_connections, tls)` has no six-argument/insecure fallback.
- `PostgresPoolConfig::from_lookup(app, get)` requires `POSTGRES_HOST`, `POSTGRES_DATABASE`, `POSTGRES_USERNAME`, and exactly one password source. Optional `POSTGRES_PORT` defaults 5432; `POSTGRES_MAX_CONNECTIONS` defaults 2. Both must be positive and fit u16/u32.
- Password sources: `POSTGRES_PASSWORD` or `POSTGRES_PASSWORD_FILE`, mutually exclusive even if empty. File maximum 16 KiB; Unix mode exactly 0400/0600. Non-Unix secret files rejected. Strip one terminal LF/CRLF only; keep meaningful spaces. Password must be nonblank UTF-8 without control characters.
- Files must be regular/nonempty/bounded. Unix opens reject final symlinks and do not block on FIFOs. Parent directories/mounts remain operator-trusted. CA is public; owner-only permissions not required.
- Host must be explicit TCP DNS/IP, not Unix socket. Database/username must be nonblank, at most 63 UTF-8 bytes, without control characters.
- `PostgresPoolConfig::from_url(url, max, tls)` accepts only `postgres://` or `postgresql://` with explicit host/user/password/database. Port defaults 5432; no implicit localhost or pgpass credentials. ASCII URL maximum 64 KiB; non-ASCII component bytes must be percent encoded.
- URL query allowlist: `sslmode`, `application_name`, once each, exactly matching protected policy. All other options/aliases rejected, including CA/client cert/key, options, and connection-field overrides. No fragments or multi-segment database path.

## SQLx and Logging Limits

- Architecture exception (ADR owned by integrator): SQLx 0.9 `new_without_pgpass` transiently reads PG* defaults. This adapter is not environment-free. Snapshot once; override host/port/database/user/password/mode/CA/app explicitly and reject all unsupported inherited settings before use. Never read `.pgpass`. Cached options clone without rebuilding or rereading inputs.
- Reject any `PGSSLCERT`, `PGSSLKEY`, `PGSSLROOTCERT`, or `PGOPTIONS` from lookup, including empty values. Direct pool constructors also inspect SQLx's snapshot and reject these inherited inputs before use. No client-cert support.
- SQLx has no client-cert getters/resetters. Inspect its documented URL projection with fixed safe authority fields; never log or parse that temporary URL. Recheck this workaround on SQLx upgrades.
- SQLx's current `tls-rustls` feature adds supplied CA to WebPKI roots; explicit CA input is not exclusive CA pinning.
- Config Debug omits usernames/passwords/URLs/root bytes/paths/app names. SQL statement and slow-statement logging disabled.
- `connect()` and `connect_session()` return opaque `PostgresConnectError`. Every exposed source-chain member has safe Debug/Display. Private redacted cause retains original SQLx error for internal classification but has `source() == None`; no raw accessor. Authentication/configuration/TLS/I/O/timeout/pool/server/protocol messages contain no provider body or credential. Do not log raw `PgConnectOptions` or pools.
- `connect_session(&self) -> Result<PgConnection, PostgresConnectError>` opens one dedicated connection directly from cached options. Same five-second constant as pool acquire timeout bounds TCP/TLS/authentication establishment. Deadline failure has the safe connection-timeout category; elapsed/SQLx cause stays private.
- Dedicated sessions are not transaction-pool connections and are not constrained by pool max_connections. Caller budgets the extra database sessions, owns close/lifetime, and sets subsequent query/lock/migration deadlines. Use this boundary instead of raw SQLx connect calls; `connect_options()` remains available but has no deadline/error wrapping itself.
- Config constructors return `PostgresPoolConfigError`. No raw input values in config errors.
- `PostgresPoolConfig` remains Clone, not Eq/PartialEq; options snapshot is opaque. `port()` is no longer const.
- Old `PostgresConnectError::Connect(sqlx_error)` call sites must use returned connect error or `PostgresConnectError::from(sqlx_error)`.

## Business Schema Gate

- Production export: `verify_business_schema(&PgPool) -> Result<(), PostgresSchemaError>`. API/worker call once before accepting work; wiring belongs to runtime owners. No config/TLS changes.
- Checks exact up-migration versions/success/SHA-384 checksums from `sqlx::migrate!("../../migrations")` against `public._sqlx_migrations`. Missing, dirty, changed, extra/unknown history fails closed. A table without history never authorizes stamping. History descriptions/timings are not schema identity.
- Requires installed `pg_trgm`, `unaccent`, `pg_ttl_index` in `public` and all 33 baseline business tables as persistent ordinary/partitioned tables. Ledger must be a persistent ordinary table without RLS; views/RLS cannot hide history. No business rows read.
- One read-only repeatable-read transaction makes catalog/history checks coherent. No DDL, migration lock, `run`, `ensure_migrations_table`, repair, stamp, or business/history mutation. Five-second total deadline, two-second statements, 500ms lock waits, five-second idle-transaction timeout. Fixed catalog search path; ledger always schema-qualified.
- One pool connection is checked out and closed, not returned, even on failure/cancellation. SQLx close-on-drop cleanup is asynchronous with its own five-second bound; server statement/idle deadlines also bound abandoned work. Successful verification commits the read-only transaction and awaits close within the total deadline.
- Operator supplies an existing shared-config pool for the business database, public-schema USAGE, catalog access and ledger SELECT. No business SELECT or write/DDL grants needed for the gate. Catalog owners and database administrators remain trusted.
- `PostgresSchemaError::code()` exposes `SCHEMA_HISTORY_MISSING`, `SCHEMA_HISTORY_DIRTY`, `SCHEMA_HISTORY_MISMATCH`, `SCHEMA_HISTORY_UNKNOWN`, `SCHEMA_HISTORY_INVALID`, `SCHEMA_EXTENSION_MISSING`, `SCHEMA_BASELINE_MISSING`, `SCHEMA_PERMISSION_DENIED`, `SCHEMA_TIMEOUT`, `SCHEMA_DEPENDENCY_UNAVAILABLE`, or `SCHEMA_EXPECTATION_INVALID`. Technical causes retained privately; all exposed Debug/Display/source members redact provider data.
- Deliberate task03 limit: unknown future migrations block old binaries, even claimed additive ones. Task05 must review an explicit compatible-superset protocol; never infer compatibility from SQL/history alone. This is point-in-time history/availability verification, not full schema drift, extension-version/preload/worker-health proof, or serialization with a concurrent migrator.
- `schema_tests.rs`/`schema_fixture.rs` stay private. Reuse `docker_fixture.rs` unchanged, cached `test-api/postgres/image-ref.txt`, isolated tmpfs/loopback fixtures, ID-only cleanup and redacted output. Test fixture alone provisions extensions/runs migrations; the shared test-api replay fixture does not create SQLx history.
- Task05 pinned-source evidence and implementation limits: `SCHEMA_GATE_05.md`.

## Integration Handoff

- Runtime/CDK constructor migration belongs to integrator; this crate change alone does not complete task02. Remove legacy implicit/insecure startup paths there; do not enable deployment as part of this slice.
- Operator supplies stage/mode, hostname matching certificate SAN, CA file, database/user, one password source, and nonsecret application label. Local runtimes must explicitly opt into disable.
- Root proposal: pin SQLx `version = "=0.9.0"` with existing features. Hoist `rustls = { version = "=0.23.43", default-features = false, features = ["std"] }` from crate to workspace if desired.
- Root lock owner must review added platform-postgres edges: `libc`, `percent-encoding`, `rstest`, `rustls 0.23.43`, `url`. No new package versions needed versus baseline lock.

## Ownership

- This doc rule `src/platform-postgres/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check --locked --offline -p platform-postgres --all-targets --all-features`
- `cargo test --locked --offline -p platform-postgres --all-features --lib`
- `cargo clippy --locked --offline -p platform-postgres --all-targets --all-features -- -D warnings`
- TLS transport: `cargo test --locked --offline -p platform-postgres --lib tls_tests::should_ -- --ignored --test-threads=1`
- Schema gate + read-only SQLx lock probe: `cargo test --locked --offline -p platform-postgres --all-features --lib schema::tests::should_ -- --ignored --test-threads=1`. Requires cached image named by `src/test-api/postgres/image-ref.txt`; no pull/override. Every successful test requires owned-ID cleanup. Guard's fake tests compile twice deliberately; test-only `duplicate_mod` expectation preserves existing fixture ownership.
- Bound each targeted invocation to 120 seconds. No workspace full-suite retry.
- Unit CA tests need `openssl` and `timeout`; Unix FIFO test also needs `mkfifo`. Keys generated under crate `target/` and removed, never committed.
- TLS tests are opt-in; require `/usr/bin/docker`, `/usr/bin/timeout`, local Unix socket `/var/run/docker.sock`, and cached `postgres:17`. No image pulls. CLI forces this Unix endpoint and isolated config directory; child environment cleared, so inherited Docker host/context/TLS settings cannot select remote Docker.
- Own unique bridge networks/containers, loopback-only published ports, tmpfs data, ephemeral CA/server keys/passwords. Separate container create/start. Guard acquires ownership only from successful creation returning one validated full Docker ID. Cleanup targets only those IDs, never intended names or volumes. Failed/ambiguous creation cannot authorize cleanup; possible unidentified leftovers require operator review, not guessed deletion. Command/provider output suppressed; successful real tests require successful cleanup.
- Fake Docker command tests need `/usr/bin/python3`; use isolated scripts, no daemon. Cover network/container collisions, failure output containing foreign IDs, invalid IDs, failed start, idempotent/failed cleanup, Unix socket validation, and hostile Docker environment.
- Tests cover real structured+URL TLS success, wrong hostname, untrusted CA, expired server cert, wrong password, plaintext rejection, and explicit test plaintext success. Dedicated session tests prove verified TLS/application name, independent sessions, rejected hostname/CA/password, and a real stalled loopback socket returning the safe timeout after five seconds. Every error-chain member checked against canaries, including actual submitted wrong password and provider authentication text. Ambient subprocess tests include empty values and typed/URL/lookup consistency; snapshot tests mutate lookup inputs/files/returned clones, never global environment. Ignored subprocess helpers run through ordinary parent tests; never run all ignored tests unfiltered.

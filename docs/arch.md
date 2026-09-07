# Architecture Guide

This document defines the default architecture and implementation conventions for this workspace. It is intended to be read before adding or changing domain logic, use cases, persistence, APIs, integrations, or projections.

Changes that intentionally deviate from this document MUST explain the reason in the pull request and SHOULD update this document when the deviation represents a new general rule.

---

## 1. Normative language

The terms **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

- **MUST / MUST NOT**: required for architectural consistency.
- **SHOULD / SHOULD NOT**: strong default; deviations require a concrete reason.
- **MAY**: optional.
- Examples illustrate the rules but do not override them.

---

## 2. Architecture at a glance

The workspace follows Domain-Driven Design, clean dependency boundaries, ports and adapters, and command/query separation.

```text
Transport
REST controllers, workers, CLI
        │
        │ calls inbound use-case contracts
        ▼
Application / service
use-case requests, commands, results, views, errors
use-case handler implementations
outbound capability and transaction ports
        │
        │ calls outbound ports
        ▼
Adapters
PostgreSQL, search engines, key-value stores,
graph/knowledge stores, external APIs, queues
        │
        ▼
External systems
```

The write model is authoritative in PostgreSQL unless a bounded context explicitly documents another operational source of truth.

Additional data sources are adapters. They MAY provide:

- search;
- fast key-value reads;
- graph or semantic enrichment;
- user-specific state;
- analytics;
- recommendations;
- external metadata;
- rebuildable read projections.

They MUST NOT silently become authoritative for domain invariants.

### Core rules

1. Controllers call use cases.
2. Use-case contracts, input/output types, and handler implementations belong to the corresponding `<entity>-service` crate.
3. Domain behavior belongs to the corresponding `<entity>-core` crate.
4. Ports describe application capabilities, not databases.
5. Adapter crates implement service-owned ports.
6. Handlers depend only on core types and service-owned ports; they MUST NOT import SQLx, database clients, SDK clients, rows, documents, or adapter implementations.
7. Storage representations remain private to their adapter crate.
8. Repositories reconstruct and persist aggregates.
9. Readers build read models.
10. Read models are not aggregates.
11. Write handlers define transaction scope through an abstract `UnitOfWork` and transaction-bound repository factories.
12. Several PostgreSQL repositories MAY participate in the same abstract transaction.
13. Cross-datasource writes do not share a transaction.
14. Projection and CDC behavior follows the dedicated CDC architecture documentation.
15. REST DTOs belong to the REST layer and are mapped by controllers.
16. Trusted caller identity is mapped into a service-owned `OperationContext`.
17. Domain and service crates MUST NOT depend on infrastructure crates.

## 3. Canonical workspace layout

Each entity is split into separate root-level workspace crates. Only adapters that the entity actually uses need to exist.

```text
Cargo.toml

record-core/
record-service/
record-postgres/
record-opensearch/

workspace-core/
workspace-service/
workspace-postgres/

platform-postgres/      # shared concrete SQLx transaction primitives when required
api/
runtime/                # composition root and process startup
```

The neutral `record` example corresponds to concrete crate families such as:

```text
product-listing-core
product-listing-service
product-listing-postgres
product-listing-opensearch
```

### 3.1 Core crate

```text
record-core/
└── src/
    ├── lib.rs
    ├── record.rs
    ├── record_id.rs
    ├── workspace_id.rs
    ├── value_objects.rs
    ├── events.rs              # optional
    ├── policies.rs
    └── errors.rs
```

`record-core` owns domain state and behavior. It MUST NOT depend on `record-service` or any adapter crate.

### 3.2 Service crate

```text
record-service/
└── src/
    ├── lib.rs
    ├── operation_context.rs
    ├── transaction.rs
    ├── use_cases/
    │   ├── mod.rs
    │   ├── commands/
    │   │   ├── create_record.rs
    │   │   ├── rename_record.rs
    │   │   └── archive_record.rs
    │   └── queries/
    │       ├── search_records.rs
    │       └── get_record_details.rs
    └── ports/
        ├── record_repository.rs
        ├── record_search_reader.rs
        ├── record_details_reader.rs
        ├── record_user_state_reader.rs
        └── record_metadata_reader.rs
```

Each use-case file SHOULD contain:

- the command or request;
- the result or final view;
- the use-case error;
- the inbound use-case trait;
- the concrete handler implementation.

`record-service` depends on `record-core` and public contracts from other core/service crates when a use case genuinely spans entities.

### 3.3 PostgreSQL adapter crate

```text
record-postgres/
└── src/
    ├── lib.rs
    ├── repository_factory.rs
    ├── repositories/
    │   └── record_repository.rs
    ├── readers/
    │   ├── record_details_reader.rs
    │   └── record_user_state_reader.rs
    ├── rows/
    │   ├── record_row.rs
    │   └── record_details_row.rs
    └── mapping.rs
```

`record-postgres` owns SQLx rows, SQL, mappings, transaction-bound repository implementations, reader implementations, and the concrete factories required by the composition root.

It MUST NOT own use-case handlers.

### 3.4 Other adapter crates

```text
record-opensearch/
└── src/
    ├── lib.rs
    ├── record_document.rs
    ├── record_search_reader.rs
    └── projector.rs
```

Technology-specific names are appropriate for adapter crates because they describe the implementation boundary.

### 3.5 Transport and composition root

REST code lives in the API crate. The current canonical REST runtime is `aura-historia-api`, an axum process without API Gateway adapters:

```text
aura-historia-api/
└── src/
    ├── main.rs              # logging, config, shutdown
    ├── lib.rs               # router, server, composition root
    ├── state.rs             # axum application state
    ├── error.rs             # problem+json API errors
    ├── auth/                # transport authentication
    └── <unit>/
        ├── mod.rs           # unit module exports
        ├── types.rs         # shared REST enum/value DTOs for this unit only
        └── <endpoint>.rs    # one controller endpoint plus unit tests
```

Each durable route unit SHOULD have its own module, for example `listing_sources/`. Each endpoint SHOULD live in one file, for example `listing_sources/get_listing_source.rs`. Unit tests for that endpoint SHOULD live in that endpoint file. Shared public response payloads that are used by many endpoints MAY live in a focused file such as `listing_sources/listing_source_data.rs`; keep shared enum/value DTOs in `listing_sources/types.rs`.

The runtime/composition-root crate constructs concrete adapters and injects them into service-owned handlers:

```text
runtime/
└── src/
    ├── main.rs
    └── wiring/
        └── record.rs
```

The composition root MAY depend on every crate required to assemble the process. It MUST NOT contain business behavior. API and worker composition-root crates MUST NOT implement service-owned inbound or outbound ports, readers, repositories, writers, senders, or use cases. They map transport/runtime inputs and compose concrete adapter crates. Transport- or runtime-local traits are allowed.

In `aura-historia-api`, concrete adapter wiring belongs in `lib.rs` or a dedicated wiring module. Route files MUST receive use-case trait objects through `state.rs`; they MUST NOT construct repositories, readers, SQL clients, or AWS clients. Route files authenticate and map only; protected endpoint authorization policies MUST live inside service use cases or service-owned policies, not in controllers.

`aura-historia-cron` is the canonical scheduled runtime. It registers UTC timer triggers and invokes service-owned use cases; scheduler libraries never own idempotency, transactions, checkpoints, or business retries. The runtime owns local overlap prevention, execution timeout/panic containment, shutdown draining, and health/readiness only.

### 3.6 Dependency direction

```text
record-core
    ▲
    │
record-service
    ▲
    │
record-postgres / record-opensearch
    ▲
    │
api and runtime
```

Allowed dependencies:

```text
record-service       -> record-core
record-postgres      -> record-service + record-core + platform-postgres as needed
record-opensearch      -> record-service + record-core identifiers as needed
api                  -> record-service + public core identifiers/value objects as needed
runtime              -> service crates + adapter crates + platform crates
```

Forbidden dependencies:

```text
record-core          -X-> record-service
record-core          -X-> adapters
record-service       -X-> adapters
record-service       -X-> REST DTOs
adapter A            -X-> private types from adapter B
controller           -X-> concrete database client
controller           -X-> repository
controller           -X-> storage row/document/item
```

Cross-entity use cases MUST have one clear owning service crate. If no existing entity service is a natural owner, create a dedicated application/service crate rather than introducing cyclic dependencies.

### 3.7 Durable shared crates

The workspace has narrow shared owners. They are not a replacement `common` hub:

- `application` owns technology-neutral application contracts such as `OperationContext`, `UnitOfWork`, `PatchField`, pagination, personalization, and boxed application errors.
- `domain-primitives` owns proven domain-neutral values, query/sort types, event IDs, version wrappers, and newtype machinery.
- `credential-core` owns credential identifiers and scope vocabulary.
- `money` and `localization` own pure currency/amount/price and language/localized values.
- `platform-postgres`, `platform-opensearch`, and `platform-observability` own concrete SQLx transaction mechanics, generic OpenSearch protocol envelopes, and subscriber setup.

Canonical core, service, adapter, runtime, and transport crates MUST import these owners directly. PostgreSQL adapters own SQLx rows and mappings; OpenSearch adapters own documents, queries, and mappings while `platform-opensearch` owns only proven generic protocol envelopes; provider crates own wire DTOs and provider vocabulary. `large-language-model` owns LLM operation/model/tier vocabulary and invocation metrics, while `platform-observability` owns subscriber setup only. `common` remains only for legacy compatibility and MUST NOT gain new canonical consumers. The canonical runtime leaves use `platform-observability` for logging, `platform-postgres` for typed pool and transaction mechanics, `application` for operation context and transaction contracts, and bounded-context core crates for identifiers. No canonical production crate may retain a normal or development `common` edge; legacy APIs, entities, Lambdas, and explicitly dual test/composition roots may remain. A shared crate MUST stay narrow, technology-neutral where named as such, and free of bounded-context storage or transport representations.

## 4. Domain-Driven Design boundaries

### 4.1 Aggregates

An aggregate is the consistency boundary for synchronous domain rules.

An aggregate MUST:

- own its invariants;
- expose behavior through methods;
- keep fields private;
- prevent invalid state transitions;
- refer to other aggregates by typed identifier.

An aggregate MAY emit domain events for meaningful completed state changes. Domain events are an optional modeling mechanism, not a requirement for every aggregate.

```rust
pub struct Record {
    id: RecordId,
    workspace_id: WorkspaceId,
    title: RecordTitle,
    status: RecordStatus,
}

impl Record {
    pub fn rename(
        &mut self,
        new_title: RecordTitle,
    ) -> Result<(), RenameRecordError> {
        if self.status.is_archived() {
            return Err(RenameRecordError::Archived);
        }

        if self.title == new_title {
            return Ok(());
        }

        self.title = new_title;

        Ok(())
    }
}
```

An event-driven aggregate MAY additionally collect domain events internally:

```rust
self.pending_events.push(RecordEvent::Renamed {
    record_id: self.id,
});
```

Non-event-driven aggregates MUST NOT introduce event machinery merely for architectural uniformity.

An aggregate MUST NOT contain a hydrated object from another aggregate:

```rust
// Forbidden
pub struct Record {
    workspace: Workspace,
}
```

It stores the reference instead:

```rust
pub struct Record {
    workspace_id: WorkspaceId,
}
```

Hydrated cross-aggregate information belongs to read models.

#### Operational metadata

Operational metadata such as `created_at`, `updated_at` is not aggregate state unless a domain invariant explicitly depends on it.

It MUST NOT be added to an aggregate merely for audit, display, sorting, or transport compatibility.

This metadata lives in the persistence layer. The repository owns writing and updating it from service-provided write metadata, clocks, and persistence defaults. When reconstructing an aggregate, the repository MUST map only domain state back into the aggregate.

Access to operational metadata belongs to dedicated readers and read use cases. A details, audit, or history reader MAY return metadata in an application-owned read model. The aggregate repository MUST NOT expose metadata just to satisfy presentation needs.

### 4.2 Domain types

`core` owns:

- aggregates;
- entities internal to aggregates;
- value objects;
- typed identifiers;
- domain policies that are pure;
- domain events, when the aggregate uses them;
- domain errors.

`core` MUST NOT depend on:

- SQLx;
- Serde solely for transport or persistence;
- HTTP frameworks;
- search clients;
- cloud SDKs;
- queue clients;
- `tracing`;
- environment variables;
- database rows or documents.

Domain code SHOULD be deterministic and testable without mocks, databases, clocks, networks, or runtimes. Time, identifiers, randomness, and external decisions MUST be supplied as values or through explicit application ports when needed.

### 4.3 Semantic absence and orthogonal dimensions

A valid absence of a semantic assertion MUST use `Option<T>`, not an `Unknown` enum variant. Uncertainty, unsupported external values, and extraction failure belong to the boundary adapter and MUST NOT be persisted as invented core truth. Orthogonal domain dimensions, such as catalog lifecycle and availability, MUST use separate types. A broad classification derived from a concrete domain value MAY be exposed for reads and queries, but MUST NOT be independently mutable or persisted.

## 5. Type ownership and visibility

### 5.1 Ownership table

| Type category | Example | Owner | Default visibility |
|---|---|---|---|
| Aggregate | `Record` | `record-core` | `pub`, fields private |
| Value object | `RecordTitle` | `record-core` | `pub`, fields private |
| Typed ID | `RecordId` | `record-core` or shared identifiers crate | `pub` |
| Domain event, when used | `RecordEvent` | `record-core` | private, `pub(crate)`, or `pub` only when consumed across crates |
| Principal/context | `Principal`, `OperationContext` | `record-service` or shared application crate | `pub` |
| Use-case command | `RenameRecordCommand` | `record-service::use_cases` | `pub` |
| Query request | `SearchRecordsRequest` | `record-service::use_cases` | `pub` |
| Use-case result | `RenameRecordResult` | `record-service::use_cases` | `pub` |
| Read model/view | `RecordSummary` | `record-service::use_cases` or `ports` | `pub` when an adapter/controller consumes it |
| Inbound use-case trait | `RenameRecordUseCase` | `record-service::use_cases` | `pub` |
| Use-case handler | `RenameRecordHandler` | `record-service::use_cases` | `pub` when wired externally; fields private |
| Outbound port | `RecordRepository`, `RecordDetailsReader` | `record-service::ports` | `pub` because adapter crates implement it |
| Transaction abstraction | `UnitOfWork`, `Transaction` | service/shared application crate | `pub` |
| PostgreSQL factory | `SqlxRecordRepositoryFactory` | `record-postgres` | `pub` when required by runtime wiring |
| PostgreSQL scoped repository | `SqlxRecordRepository` | `record-postgres` | private whenever the factory return type can remain opaque |
| PostgreSQL row | `RecordRow` | `record-postgres` | private or `pub(crate)` |
| Search document | `RecordDocument` | `record-opensearch` | private or `pub(crate)` |
| Shared search protocol envelope | `SearchResponse<T>` | `platform-opensearch` | `pub` only after multiple production adapters prove the exact generic wire shape |
| REST request DTO | `RenameRecordRequestDto` | `api` | private or `pub(crate)` |
| REST response DTO | `RecordDetailsResponseDto` | `api` | private or `pub(crate)` |

### 5.1.1 Semantic leaf types across boundaries

Architectural ownership of a DTO, command, read model, row, or document does not mean every field needs a layer-local type. Boundary structures SHOULD reuse public canonical identifiers, value objects, and enums when meaning and the intended value set are identical, reuse preserves dependency direction, and no technology representation escapes its owner.

A boundary-local field type SHOULD exist only for a distinct vocabulary, compatibility rule, validation rule, unknown-value policy, representation metadata, or independent lifecycle. A variant-for-variant mirror enum with only identity conversions is a smell and needs a concrete reason.

REST owns request/response structure, JSON field names, omission and null rules, aliases, REST-specific compatibility vocabulary, and HTTP validation. A semantic owner MAY also own a stable canonical machine identifier when that identifier is genuinely shared across first-party representations. REST may expose that identifier through an API-local codec without adding Serde to core types; if REST vocabulary diverges, the API-local codec owns the divergent mapping. REST does not own a duplicate of every semantic field.

PostgreSQL owns row types and storage encoding. A row MAY decode directly into canonical semantic leaf types; an intermediate storage enum is unnecessary unless it carries distinct storage semantics.

```rust
#[derive(serde::Deserialize)]
struct CreateListingSourceDto {
    name: ListingSourceName,
    #[serde(with = "ingestion_method_wire")]
    ingestion_methods: Vec<ListingIngestionMethod>,
}
```

Avoid a boundary mirror such as `ListingIngestionMethodDto` plus exhaustive identity conversions unless an intentional contract divergence requires it.

For bounded-context enums, this workspace uses the policy that the canonical value set equals the REST value set when the API delegates to the canonical identifier. Adding a canonical variant therefore intentionally makes it REST-visible. A REST-specific alias, subset, or compatibility value still requires an API-local codec and review.

### 5.2 Visibility rules

Use the narrowest visibility that satisfies a real production crate boundary.

- Items are private by default.
- Aggregate fields MUST be private.
- Use `pub(super)` only for a parent module inside the same crate.
- Use `pub(crate)` only for cross-module access inside the same crate.
- Use `pub` only when another production crate must use or implement the item.
- Service-owned ports MUST be `pub` because adapter crates implement them.
- Use-case handlers and constructors MUST be `pub` only when the composition root constructs them directly.
- Concrete adapter factories/readers MUST be `pub` only when the composition root or a black-box consumer needs them.
- Adapter rows, mapping helpers, SQL parameter structs, concrete transaction-scoped repositories, and adapter-specific client response types MUST remain private or `pub(crate)`. A narrow platform crate MAY expose a generic protocol envelope only when multiple production adapters require the exact shape and no bounded-context storage representation escapes.
- Fields of public adapter types MUST remain private.
- Do not expose a public constructor for a type that consumers should obtain only through a factory.
- Do not widen visibility solely for tests.

A public item is part of the workspace architecture contract even when the workspace is not published to crates.io.

### 5.3 Opaque transaction-scoped implementations

Repository factories SHOULD use return-position `impl Trait` so the concrete transaction-scoped repository can remain private:

```rust
pub trait RecordRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl RecordRepository + 'tx;
}
```

The adapter may then keep the implementation private:

```rust
pub struct SqlxRecordRepositoryFactory;

struct SqlxRecordRepository<'tx> {
    tx: &'tx mut SqlxTransaction,
}
```

If the workspace MSRV prevents opaque return types and a public associated type is unavoidable, expose only the minimum required type, annotate it `#[doc(hidden)]`, keep all fields private, and provide no public constructor.

### 5.4 Rehydration boundary

Because the PostgreSQL adapter is a separate crate, aggregate rehydration APIs used by adapters must be deliberately `pub`.

```rust
impl Record {
    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedRecordState) -> Result<Self, RehydrateRecordError> {
        // Validate persisted state without emitting new events.
        todo!()
    }
}
```

This is an adapter-facing construction boundary, not a general mutation API. Its input fields SHOULD use domain types where practical, and all aggregate fields remain private.

### 5.5 Test visibility

Tests inside a source file under `#[cfg(test)] mod tests` can access that file's private items and MAY run against real infrastructure.

Tests in a crate-level `tests/` directory compile as separate crates and MUST use only the deliberate public API.

Therefore:

- private implementation and real-infrastructure adapter tests belong beside the implementation;
- black-box contract tests belong in `/tests`;
- implementation details MUST NOT be made `pub` merely so a `/tests` test can access them.

## 6. Use cases

Reads and writes are both use cases.

Each use case SHOULD have its own file in the corresponding service crate. The file owns:

- command or request;
- result or final view;
- use-case error;
- inbound use-case trait;
- the concrete handler implementation.

### 6.1 Write use-case contract

```rust
// record-service/src/use_cases/commands/rename_record.rs

pub struct RenameRecordCommand {
    pub record_id: RecordId,
    pub new_title: String,
}

pub struct RenameRecordResult {
    pub record_id: RecordId,
    pub title: String,
}

#[derive(Debug, thiserror::Error)]
pub enum RenameRecordError {
    #[error("record not found")]
    NotFound,

    #[error("concurrent record update")]
    ConcurrencyConflict,

    #[error("invalid title")]
    InvalidTitle,

    #[error("authenticated actor required")]
    AuthenticatedActorRequired,

    #[error("temporary persistence failure")]
    TemporarilyUnavailable,

    #[error("internal failure")]
    Internal,
}

#[async_trait::async_trait]
pub trait RenameRecordUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: RenameRecordCommand,
    ) -> Result<RenameRecordResult, RenameRecordError>;
}
```

Commands SHOULD express business intent:

```text
CreateRecord
RenameRecord
ArchiveRecord
RestoreRecord
PublishRecord
```

Avoid generic update commands with weak intent:

```text
UpdateRecord {
    title: Option<String>,
    status: Option<String>,
    owner: Option<Uuid>,
    ...
}
```

When a public PATCH endpoint is intentionally broad, the service use case is still named `Update*`. Use a service-owned `Update*Command` with shared tri-state `application::patch_field::PatchField`: `Unchanged`, `Set(value)`, and `Clear`. The helper belongs to the narrow `application` crate, but the command belongs in `service`, not in `core`.

The update handler MUST translate command fields into explicit aggregate methods such as `change_title`, `replace_address`, or `replace_contact`, and track `ChangeOutcome`. Generic update MUST NOT include state-machine transitions that deserve their own use case, such as publishing, archiving, or changing aggregate status.

A broad update use case is one logical write. Domain methods return `ChangeOutcome` and MUST NOT know about storage versions. If nothing changed, the handler MUST NOT execute a persistence update. If anything changed, the handler calls repository `update(&aggregate, loaded.version)` once. The PostgreSQL repository writes complete authoritative state, enforces optimistic concurrency with `WHERE version = $expected_version`, and increments `version` exactly once with `version = version + 1`. The new version is internal PostgreSQL/CDC state and MUST NOT be returned from ordinary use cases.

### 6.2 Read use-case contract

```rust
// record-service/src/use_cases/queries/search_records.rs

pub struct SearchRecordsRequest {
    pub text: String,
    pub page: PageRequest,
}

pub struct RecordSummary {
    pub record_id: RecordId,
    pub title: String,
    pub container_name: String,
    pub is_watched: bool,
    pub is_liked: bool,
}

pub struct SearchRecordsResult {
    pub items: Vec<RecordSummary>,
    pub total: u64,
    pub next_page: Option<PageToken>,
}

#[async_trait::async_trait]
pub trait SearchRecordsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchRecordsRequest,
    ) -> Result<SearchRecordsResult, SearchRecordsError>;
}
```

The final read model is owned by the use case, not by any data source.

### 6.3 One implementation per use case

A use case SHOULD have one focused handler implementation in the service crate.

Preferred:

```text
RenameRecordUseCase
    implemented by RenameRecordHandler

SearchRecordsUseCase
    implemented by SearchRecordsHandler

GetRecordDetailsUseCase
    implemented by GetRecordDetailsHandler
```

Avoid one large implementation:

```rust
// Forbidden direction
pub struct RecordService<
    Repository,
    Search,
    Details,
    Metadata,
    UserState,
    ...
> {
    // dependencies for every unrelated use case
}
```

Each handler MUST depend only on the capabilities it uses.

### 6.4 Handler location and dependencies

All use-case handler implementations live in the corresponding `<entity>-service` crate.

```text
record-service/src/use_cases/commands/rename_record.rs
    RenameRecordCommand
    RenameRecordResult
    RenameRecordError
    RenameRecordUseCase
    RenameRecordHandler
```

Handlers MUST work exclusively against:

- core types;
- service-owned repository/reader/writer ports;
- service-owned transaction abstractions;
- public contracts from another service/core crate when the use case genuinely spans entities.

Handlers MUST NOT import:

```text
sqlx
PgPool
PgConnection
OpenSearch clients
adapter rows/documents
concrete adapter factories
```

This gives one unambiguous rule: service crates implement application behavior; adapter crates implement infrastructure ports.

## 7. Inbound use-case traits and outbound ports

There are two directions of traits.

```text
REST/controller
    │
    │ calls inbound port
    ▼
RenameRecordUseCase
    │
    │ implemented in record-service
    ▼
RenameRecordHandler
    │
    │ calls outbound ports
    ▼
UnitOfWork + RecordRepositoryFactory + authorization readers
    │
    │ implemented by adapter crates
    ▼
PostgreSQL and other infrastructure
```

### Inbound use-case traits

Inbound use-case traits describe what callers may do.

Examples:

```text
CreateRecordUseCase
RenameRecordUseCase
SearchRecordsUseCase
GetRecordDetailsUseCase
```

They belong to the corresponding service crate.

Controllers MUST depend on inbound use-case traits, never concrete handlers or adapters.

### Outbound ports

Outbound ports describe capabilities needed by handlers.

Examples:

```text
RecordRepository
RecordRepositoryFactory
RecordSearchReader
RecordDetailsReader
RecordUserStateReader
RecordMetadataReader
UnitOfWork
Clock
AuthorizationPolicy
IdempotencyStore
```

They belong to service crates or a small shared application crate for genuinely cross-cutting abstractions.

A neutral reusable capability crate MAY own a technology-neutral contract plus its provider implementation when it has no bounded-context types. For example, `embedding` may own embedding generation, `large-language-model` may own typed structured generation, and `image-fetcher` may own safe external-image retrieval. The consuming service owns semantic fields, application response types and schemas, result mapping, concurrency, business retry policy, and application-specific input limits. A capability with explicit semantic operations owns provider/model-specific prompt or request encoding; callers MUST NOT construct provider-recommended instruction strings. Provider/model selection belongs to composition and provider configuration, not a service request: a configured provider implementation may be injected separately for each use case. Provider implementations own provider authentication, protocol, configured model identifiers, media preparation, transport timeouts, provider error classification, and provider-specific prompt format. Such a crate MUST NOT import an entity core/service crate or contain entity-specific behavior.

Ports MUST be named by capability, not by technology.

Preferred:

```text
RecordSearchReader
RecordMetadataReader
RecordUserStateReader
```

Forbidden:

```text
OpenSearchPort
PostgresReader
ExternalDatabasePort
```

One adapter may implement multiple ports. One port may have multiple implementations.

```text
RecordDetailsReader
    <- PostgresRecordDetailsReader

RecordSearchReader
    <- OpenSearchRecordSearchReader
    <- PostgresRecordSearchReader
```

There is not one port per data source.

Readers and repositories SHOULD receive only the narrow application data they require. They MUST NOT receive `OperationContext` or transport DTOs.

## 8. Repositories

### 8.1 Responsibility

A repository reconstructs and persists an aggregate.

```rust
pub type VersionedRecord = Versioned<Record, RecordStorageVersion>;

#[async_trait::async_trait]
pub trait RecordRepository: Send {
    async fn find_by_id(
        &mut self,
        id: RecordId,
    ) -> Result<Option<VersionedRecord>, RecordRepositoryError>;

    async fn insert(
        &mut self,
        record: &Record,
    ) -> Result<VersionedRecord, RecordRepositoryError>;

    async fn update(
        &mut self,
        record: &Record,
        expected_version: RecordStorageVersion,
    ) -> Result<VersionedRecord, RecordRepositoryError>;
}
```

The repository port is public because a separate adapter crate implements it.

Repository `insert` and `update` methods MUST return the persisted aggregate state, not `()`. When storage-generated metadata such as `created`, `updated`, or version is needed by the use case result, return a storage-neutral persisted model that contains the aggregate plus that metadata. SQL row types MUST still stay private to the adapter.

A repository MAY contain additional aggregate-relevant lookup methods when they are needed to reconstruct or enforce the aggregate boundary.

A repository MUST NOT become a general read API.

Forbidden:

```rust
trait RecordRepository {
    async fn search(...);
    async fn get_details_with_container(...);
    async fn get_user_likes(...);
    async fn recommendations(...);
    async fn analytics_dashboard(...);
}
```

Those capabilities belong to readers.

A transaction-bound repository is obtained through a service-owned factory:

```rust
pub trait RecordRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl RecordRepository + 'tx;
}
```

The factory allows a handler to use clean repository methods while binding several repositories to the same transaction.

### 8.2 Method naming

This project prefers explicit semantics:

```text
find_by_id
insert
update
```

`find_by_id` returns `Option`.

```rust
async fn find_by_id(
    &mut self,
    id: RecordId,
) -> Result<Option<Record>, RecordRepositoryError>;
```

A method named `get_by_id` MAY be used when absence is represented as an error:

```rust
async fn get_by_id(
    &mut self,
    id: RecordId,
) -> Result<Record, GetRecordError>;
```

`insert` means the aggregate MUST be new and MUST return the inserted aggregate state.

`update` means the aggregate MUST exist, MUST enforce optimistic concurrency through an internal loaded storage version, and MUST return the updated aggregate state.

A generic `save` SHOULD NOT be used unless new/existing semantics and storage-version handling are explicit.

A generic `delete` SHOULD NOT be used for normal domain behavior. Prefer a domain state transition:

```text
archive
withdraw
disable
retire
```

Physical deletion MUST be an explicit administrative or retention operation such as:

```text
purge_expired_draft
remove_personal_data
delete_unrecoverable_record
```

### 8.3 Operational truth

The authoritative repository is backed by the operational source of truth, normally PostgreSQL.

Search indexes, key-value projections, caches, graph stores, and external metadata sources MUST NOT implement the aggregate repository unless they are explicitly the authoritative write model for that bounded context.

---

## 9. Readers and read models

Readers provide purpose-specific read capabilities.

```rust
#[async_trait::async_trait]
pub trait RecordDetailsReader: Send + Sync {
    async fn find_details(
        &self,
        record_id: RecordId,
    ) -> Result<Option<RecordBaseDetails>, RecordDetailsReadError>;
}
```

A reader returns application-owned read models:

```rust
pub struct RecordBaseDetails {
    pub record_id: RecordId,
    pub title: String,
    pub container: ContainerSummary,
}

pub struct ContainerSummary {
    pub container_id: ContainerId,
    pub name: String,
}
```

It MUST NOT return:

- domain aggregates for display;
- PostgreSQL rows;
- search documents;
- key-value items;
- external-client response types.

### 9.1 Relational joins

A normalized PostgreSQL join used for presentation belongs in a reader, not an aggregate repository.

```sql
SELECT
    r.id AS record_id,
    r.title AS record_title,
    c.id AS container_id,
    c.name AS container_name
FROM records r
JOIN containers c ON c.id = r.container_id
WHERE r.id = $1
```

The adapter may know both tables without importing another adapter's private Rust types.

SQL/schema coupling does not require a Rust dependency on another domain aggregate or its row type.

### 9.2 Hydration

Hydration is application orchestration.

Example:

```text
SearchRecordsHandler
    │
    ├── RecordSearchReader
    │       -> search source
    │       <- ordered RecordSearchHit values
    │
    └── RecordUserStateReader
            -> batch query by actor and record IDs
            <- HashMap<RecordId, RecordUserState>

SearchRecordsHandler
    -> merge while preserving search order
    -> SearchRecordsResult
```

Ports:

```rust
#[async_trait::async_trait]
pub trait RecordSearchReader: Send + Sync {
    async fn search(
        &self,
        request: &SearchRecordsRequest,
    ) -> Result<SearchResult<RecordSearchHit>, RecordSearchReadError>;
}

#[async_trait::async_trait]
pub trait RecordUserStateReader: Send + Sync {
    async fn find_for_records(
        &self,
        actor_id: ActorId,
        record_ids: &[RecordId],
    ) -> Result<HashMap<RecordId, RecordUserState>, RecordUserStateReadError>;
}
```

Rules:

- User-specific hydration MUST be batched.
- A reader MUST NOT be called once per search hit.
- Search ordering MUST be preserved after hydration.
- Missing user-state rows SHOULD map to explicit defaults.
- If user state affects filtering or ranking, it MUST be part of the query strategy rather than filtering only the returned page afterward.

### 9.3 Multiple data sources

A use case may compose any number of readers:

```text
GetRecordDetailsHandler
    ├── RecordDetailsReader        -> PostgreSQL
    ├── RecordMetadataReader       -> additional data source
    └── RecordUserStateReader      -> PostgreSQL or key-value source
```

The handler owns the final result. Keep reusable item data and orthogonal per-user state separate with the shared wrapper:

```rust
pub struct RecordDetailsView {
    pub record_id: RecordId,
    pub title: String,
    pub container: ContainerSummary,
    pub metadata: MetadataSection,
}

pub type PersonalizedRecordDetailsView =
    application::personalized::Personalized<RecordDetailsView, RecordUserState>;
```

The API maps that wrapper to a required `item` plus optional `userState`; do not inline `user_state` into the reusable item view.

Partial models belong to the ports that require them:

```text
RecordBaseDetails
RecordMetadataView
RecordUserState
```

Adapters map their private representations into these application types.

The controller MUST NOT compose data sources.

### 9.4 Required and optional enrichment

The use case MUST explicitly decide whether additional data is required.

```rust
pub enum MetadataSection {
    Available(RecordMetadataView),
    Empty,
    TemporarilyUnavailable,
}
```

Do not represent source failure as genuine absence unless the product semantics explicitly accept that loss of distinction.

---

## 10. Mapping and serialization

Mapping code belongs at the boundary that owns the source representation.

```text
REST DTO              -> controller/API mapping
PostgreSQL row        -> PostgreSQL adapter mapping
Search document       -> search adapter mapping
Key-value item        -> key-value adapter mapping
External response     -> corresponding adapter mapping
```

Storage and transport mapping MUST NOT be placed in `core`. A core crate MAY own the semantic fields of a composite domain key, but labeled string encodings for storage or transport MUST live at the owning boundary and preserve their legacy format there when compatibility requires it.

Fieldless domain enums with a stable machine-readable identity MUST own it through an explicit exhaustive `as_str()` mapping. Inverse lookup derives `EnumIter` and compares that canonical value at the owning boundary; it MUST NOT maintain a second string-to-variant mapping or infer stable identifiers from Rust variant names. Persisted reads match exactly and reject unknown or noncanonical values. A data-carrying enum uses a fieldless discriminator when inverse textual lookup is needed. Database constraints and migrations remain a separate contract and MUST evolve with canonical values.

### 10.1 REST mapping

REST request and response DTOs belong to the controller/API module.

```rust
#[derive(serde::Deserialize)]
pub(crate) struct RenameRecordRequestDto {
    pub title: String,
}

#[derive(serde::Serialize)]
pub(crate) struct RenameRecordResponseDto {
    pub id: Uuid,
    pub title: String,
}
```

The controller maps request DTOs into service-owned commands.

Use `TryFrom` when parsing or validation can fail:

```rust
impl TryFrom<(RecordId, RenameRecordRequestDto)>
    for RenameRecordCommand
{
    type Error = ApiInputError;

    fn try_from(
        value: (RecordId, RenameRecordRequestDto),
    ) -> Result<Self, Self::Error> {
        let (record_id, dto) = value;

        Ok(Self {
            record_id,
            new_title: dto.title,
        })
    }
}
```

Use `From` for infallible result-to-response conversion:

```rust
impl From<RenameRecordResult> for RenameRecordResponseDto {
    fn from(result: RenameRecordResult) -> Self {
        Self {
            id: result.record_id.into_uuid(),
            title: result.title,
        }
    }
}
```

The mapping implementation SHOULD live in:

```text
api/record/mapping.rs
```

Small mappings used by only one controller MAY live in the controller file.

REST enum/value DTOs shared by multiple endpoints in the same unit SHOULD live in that unit's `types.rs`, for example `listing_sources/types.rs`. Do not put endpoint-specific request or response DTOs there. Keep endpoint payload DTOs beside the endpoint unless they are genuinely shared public REST shapes. A unit-local `types.rs` MAY instead hold wire codecs for canonical semantic leaf types; REST keeps its wire contract without duplicating the semantic type.

The service MUST NOT know REST DTOs or HTTP status codes.

### 10.2 PostgreSQL deserialization with `FromRow`

PostgreSQL rows SHOULD use `sqlx::FromRow` for deserialization.

```rust
// record-postgres/src/rows/record_row.rs

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct RecordRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub title: String,
    pub status: String,
    pub version: i64,
}
```

`RecordRow` is a storage representation. It MUST remain private to the PostgreSQL adapter.

Deserialization flow:

```text
PostgreSQL row
    -> sqlx::FromRow
RecordRow
    -> TryFrom / adapter mapping
Record
```

Mapping to the aggregate SHOULD use `TryFrom` because persisted state may be corrupt or incompatible:

```rust
// record-postgres/src/mapping.rs

impl TryFrom<RecordRow> for Versioned<Record, RecordStorageVersion> {
    type Error = RecordRowMappingError;

    fn try_from(row: RecordRow) -> Result<Self, Self::Error> {
        let version = RecordStorageVersion::try_from(row.version)?;
        let record = Record::rehydrate(
            RecordId::from_uuid(row.id),
            WorkspaceId::from_uuid(row.workspace_id),
            RecordTitle::try_from(row.title)?,
            RecordStatus::try_from(row.status.as_str())?,
        )
        .map_err(RecordRowMappingError::InvalidPersistedState)?;

        Ok(Versioned::new(record, version))
    }
}
```

The domain MAY expose a crate-visible rehydration function:

```rust
impl Record {
    #[doc(hidden)]
    pub fn rehydrate(
        id: RecordId,
        workspace_id: WorkspaceId,
        title: RecordTitle,
        status: RecordStatus,
    ) -> Result<Self, RehydrateRecordError> {
        // Re-establish invariants without emitting new events.
        todo!()
    }
}
```

`rehydrate` MUST:

- establish all required invariants;
- not emit new domain events;
- not pretend persisted data is automatically valid.

A joined query gets its own row type:

```rust
#[derive(Debug, sqlx::FromRow)]
struct RecordDetailsRow {
    record_id: Uuid,
    record_title: String,
    container_id: Uuid,
    container_name: String,
}
```

It maps directly to the application read model:

```rust
impl From<RecordDetailsRow> for RecordBaseDetails {
    fn from(row: RecordDetailsRow) -> Self {
        Self {
            record_id: RecordId::from_uuid(row.record_id),
            title: row.record_title,
            container: ContainerSummary {
                container_id: ContainerId::from_uuid(row.container_id),
                name: row.container_name,
            },
        }
    }
}
```

Use `TryFrom` instead of `From` when mapping can fail.

### 10.3 PostgreSQL serialization

Serialization from a domain aggregate to PostgreSQL MUST occur inside the concrete repository/DAO implementation.

Preferred:

```rust
impl SqlxRecordRepository<'_> {
    async fn update(
        &mut self,
        record: &Record,
        expected_version: RecordStorageVersion,
    ) -> Result<(), RecordRepositoryError> {
        let row = sqlx::query_as::<_, (i64,)>(
            r#"
            UPDATE records
            SET
                title = $1,
                status = $2,
                version = version + 1,
                updated_at = now()
            WHERE id = $3
              AND version = $4
            RETURNING version
            "#,
        )
        .bind(record.title().as_str())
        .bind(record.status().as_db_str())
        .bind(record.id().as_uuid())
        .bind(expected_version.into_inner())
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(RecordRepositoryError::from)?;

        let Some((version,)) = row else {
            return Err(RecordRepositoryError::ConcurrencyConflict);
        };

        RecordStorageVersion::try_from(version)
            .map_err(|_| RecordRepositoryError::InvalidPersistedState)?;

        Ok(())
    }
}
```

The project SHOULD NOT define a public or cross-layer conversion such as:

```rust
impl From<&Record> for RecordRow
```

for write serialization.

Reasons:

- write parameters may differ from read rows;
- inserts and updates need different fields;
- database-specific encoding belongs to the DAO;
- a generic row conversion can conceal optimistic concurrency and generated columns.

When binding logic is repeated, use a private adapter-local helper:

```rust
struct RecordWriteParams<'a> {
    title: &'a str,
    status: &'a str,
    version: i64,
}
```

That helper MUST remain private to the PostgreSQL adapter.

### 10.4 Search document mapping

A search adapter owns its document:

```rust
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RecordDocument {
    id: String,
    title: String,
    container_name: String,
    projection_version: u64,
}
```

Read mapping:

```text
Search response JSON
    -> RecordDocument
    -> RecordSearchHit
```

Mapping used to build or update projections belongs in the source adapter and follows the CDC architecture documented in Section 12.

The search document MUST NOT escape the adapter.

### 10.5 External response mapping

An adapter for an additional data source owns the external response type.

```rust
#[derive(serde::Deserialize)]
struct ExternalMetadataResponse {
    labels: Vec<String>,
    relations: Vec<ExternalRelation>,
}
```

It maps to an application-owned model:

```rust
pub struct RecordMetadataView {
    pub labels: Vec<String>,
    pub relations: Vec<MetadataRelation>,
}
```

External-client types MUST NOT appear in service contracts.

---

## 11. Transactions

### 11.1 Ownership

The service-owned use-case handler defines transaction scope without importing SQLx.

The handler:

1. begins an abstract transaction through `UnitOfWork`;
2. binds transaction-scoped repositories/readers through factories;
3. executes domain behavior;
4. writes all authoritative state required by the use case;
5. explicitly commits the abstract transaction.

The concrete adapter implements that abstraction using SQLx.

```text
record-service
    RenameRecordHandler
        -> UnitOfWork
        -> RecordRepositoryFactory

record-postgres
    SqlxUnitOfWork
    SqlxRecordRepositoryFactory
    private SqlxRecordRepository
```

### 11.2 Transaction and unit-of-work ports

The transaction lifecycle is a service-owned contract:

```rust
#[async_trait::async_trait]
pub trait Transaction: Send {
    async fn commit(self) -> Result<(), TransactionError>;
}

#[async_trait::async_trait]
pub trait UnitOfWork: Send + Sync {
    type Tx: Transaction;

    async fn begin(&self) -> Result<Self::Tx, TransactionError>;
}
```

These abstractions expose transaction lifecycle only. They MUST NOT accumulate entity-specific repository methods.

A shared application crate MAY own these traits when several entity-service crates use the same abstraction.

### 11.3 Transaction-scoped repository factories

Repositories expose clean methods without a transaction argument. A factory binds a repository implementation to the active transaction:

```rust
pub trait RecordRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl RecordRepository + 'tx;
}
```

PostgreSQL implementation:

```rust
pub struct SqlxRecordRepositoryFactory;

struct SqlxRecordRepository<'tx> {
    tx: &'tx mut SqlxTransaction,
}

impl RecordRepositoryFactory<SqlxTransaction>
    for SqlxRecordRepositoryFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl RecordRepository + 'tx {
        SqlxRecordRepository { tx }
    }
}
```

The concrete scoped repository remains private because callers interact only through the opaque return type.

### 11.4 Chained temporary repositories

Handlers SHOULD bind and call one transaction-scoped repository in a single chain:

```rust
let Versioned {
    value: record,
    version,
} = self
    .records
    .in_transaction(&mut tx)
    .find_by_id(command.record_id)
    .await?
    .ok_or(RenameRecordError::NotFound)?;
```

The temporary repository is dropped at the semicolon, releasing its mutable borrow of the transaction.

Another repository can then use the same transaction:

```rust
let workspace = self
    .workspaces
    .in_transaction(&mut tx)
    .find_by_id(record.workspace_id())
    .await?
    .ok_or(RenameRecordError::WorkspaceNotFound)?;
```

Writes follow the same pattern:

```rust
self.records
    .in_transaction(&mut tx)
    .update(
        &record,
        version,
    )
    .await?;
```

### 11.5 Canonical service-owned write handler

```rust
pub struct RenameRecordHandler<U, R> {
    unit_of_work: U,
    records: R,
}

impl<U, R> RenameRecordHandler<U, R> {
    pub fn new(unit_of_work: U, records: R) -> Self {
        Self {
            unit_of_work,
            records,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> RenameRecordUseCase for RenameRecordHandler<U, R>
where
    U: UnitOfWork,
    R: RecordRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "rename_record",
        skip_all,
        fields(
            record_id = %command.record_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: RenameRecordCommand,
    ) -> Result<RenameRecordResult, RenameRecordError> {
        let actor = context
            .actor_label()
            .ok_or(RenameRecordError::AuthenticatedActorRequired)?;

        let mut tx = self.unit_of_work.begin().await?;

        let Versioned {
            value: mut record,
            version: loaded_version,
        } = self
            .records
            .in_transaction(&mut tx)
            .find_by_id(command.record_id)
            .await?
            .ok_or(RenameRecordError::NotFound)?;

        authorize_rename(actor, &record)?;

        let new_title = RecordTitle::try_from(command.new_title)
            .map_err(|_| RenameRecordError::InvalidTitle)?;

        let outcome = record.rename(new_title)?;

        let persisted = if outcome.changed() {
            self.records
                .in_transaction(&mut tx)
                .update(
                    &record,
                    loaded_version,
                )
                .await?
        } else {
            Versioned::new(record, loaded_version)
        };

        tx.commit().await?;

        Ok(RenameRecordResult {
            record_id: persisted.value.id(),
            title: persisted.value.title().to_string(),
        })
    }
}
```

The handler is datastore-independent and lives in `record-service`.

A successful transaction MUST end in explicit `commit().await`.

A write use case MUST NOT read after write just to build its response. The repository write result is the source for the returned command view/result. If the API needs a richer write response, make the repository return a storage-neutral persisted model with the needed metadata, or make the use-case result less rich.

An uncommitted concrete transaction that leaves scope is expected to roll back. Dropping or “closing” a transaction MUST NOT be treated as commit.

### 11.6 Multiple repositories in one transaction

A handler may use any number of repository factories whose implementations accept the same transaction type:

```rust
pub struct GrantWorkspaceAccessHandler<U, R, W, A> {
    unit_of_work: U,
    records: R,
    workspaces: W,
    access: A,
}
```

```rust
let mut tx = self.unit_of_work.begin().await?;

let record = self.records
    .in_transaction(&mut tx)
    .find_by_id(command.record_id)
    .await?
    .ok_or(Error::RecordNotFound)?;

let workspace = self.workspaces
    .in_transaction(&mut tx)
    .find_by_id(record.value.workspace_id())
    .await?
    .ok_or(Error::WorkspaceNotFound)?;

self.access
    .in_transaction(&mut tx)
    .grant(command.user_id, workspace.value.id(), &metadata)
    .await?;

tx.commit().await?;
```

All operations above participate in the same concrete PostgreSQL transaction when the runtime supplies compatible SQLx implementations.

A shared `platform-postgres` crate MAY expose the public concrete `SqlxTransaction` used by several entity-specific PostgreSQL adapter crates. Its fields and SQLx internals MUST remain private.

### 11.7 Transactional readers

A read that influences an invariant-critical write MUST use the same transaction and therefore MUST have a transaction-bound reader factory:

```rust
pub trait WorkspacePolicyReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl WorkspacePolicyReader + 'tx;
}
```

When a use case works on operational data through ports and must own the transaction, all operational ports participating in that use case MUST model the same unit of work:

- the handler depends on `UnitOfWork`;
- repository ports expose transaction-scoped factories;
- PostgreSQL reader ports expose transaction-scoped factories;
- the handler begins the transaction, obtains each port with `.in_transaction(&mut tx)`, and explicitly commits on success.

Do not call a pool-backed reader on another connection when its result must be consistent with the active transaction.

### 11.8 Ordinary readers

Ordinary presentation readers are not transaction-bound only when the use case does not need an application-owned transaction or consistent operational snapshot. Their adapter implementations MAY own a pool/client internally:

```rust
pub struct PostgresRecordDetailsReader {
    pool: sqlx::PgPool,
}
```

The service handler depends only on the public `RecordDetailsReader` trait.

A single SQL statement does not need an application-managed transaction. Use an explicit read transaction only when several SQL statements must observe one consistent snapshot; this is exceptional and MUST be documented.

### 11.9 Cross-datasource boundaries

A PostgreSQL transaction cannot atomically include:

- a search engine;
- a key-value store;
- a graph or knowledge source;
- an external API;
- a message broker without a specific transaction protocol.

A multi-source handler still lives in the service crate and composes abstract ports. It SHOULD avoid holding a PostgreSQL transaction open while waiting on a slow external source. Read external data first when safe, then open a short PostgreSQL transaction and revalidate authoritative state before writing.

Authoritative PostgreSQL writes MUST commit according to the transaction rules above. Replication and projection propagation follow the CDC architecture documentation.

## 12. CDC and projection architecture

CDC propagates committed PostgreSQL changes to workers and rebuildable read projections.

```text
PostgreSQL commit
    -> logical replication
    -> Sequin
    -> CDC router
    -> bounded worker queues
    -> projection handlers
```

### 12.1 Storage ownership

Every dataset MUST have one documented operational owner.

PostgreSQL owns business truth for:

* users;
* parties;
* listing sources;
* partnerships and partnership applications;
* product listings;
* product-listing events;
* immutable canonical FX snapshots with generation and EUR-base `units_per_eur` quotes;
* product-listing translations;
* product-listing watchlists;
* search filters;
* search-filter matches;
* notifications and notification delivery state; a Notification is separate from its one-or-more delivery rows, each uniquely identified by `(notification_id, channel, target_key)`. The application planner selects channels, each channel adapter resolves its target, and generic delivery claim/send/finalize stays outside notification producers. EMAIL is the sole production sender.
* User access tokens;
* OAuth clients;
* OAuth authorization codes;
* OAuth third-party exchange codes.

Credential tables are operational PostgreSQL storage, not Sequin sources. Expiry remains service-side correctness; `pg_ttl_index` is asynchronous physical cleanup only.

OpenSearch contains rebuildable search projections only.

`product_listing_events` is a transactional ProductListing domain/enrichment journal and direct Sequin CDC source. It is not the ProductListing aggregate source of truth, an outbox, or an event-sourcing stream. One logical ProductListing domain write appends zero or one domain payload; enrichment completions append compact provenance rows in the same authoritative transaction.

Additional stores MUST be classified as one of:

* authoritative operational storage;
* rebuildable projection;
* external source;
* cache.

A store MUST NOT become authoritative merely because it is used on a hot read path.

### 12.2 Write contract

Authoritative PostgreSQL writes occur in one SQLx transaction.

External projections MUST NOT be updated synchronously inside that transaction.

Forbidden:

```text
BEGIN PostgreSQL
update PostgreSQL
update OpenSearch
update key-value projection
COMMIT PostgreSQL
```

Required:

```text
commit PostgreSQL
    -> Sequin observes committed product_listing_events INSERT
    -> worker validates and routes CDC
    -> update projections asynchronously
```

Domain invariants MUST NOT depend on projections being current.

### 12.3 Router and acknowledgment

The CDC router converts source changes into domain-relevant jobs.

One change MAY create several jobs:

```text
source change
    -> search projection job
    -> notification job
    -> matching job
```

The router MUST:

* validate the change shape;
* derive stable job identifiers;
* enqueue all required jobs;
* apply bounded backpressure;
* acknowledge or reject the Sequin delivery.

The delivery is acknowledged only after all required jobs have been added to their bounded in-memory queues.

If any enqueue fails, the delivery MUST NOT be acknowledged.

A redelivery may therefore create duplicate jobs. All handlers MUST be idempotent.

### 12.4 MVP delivery guarantee

The MVP has no durable worker inbox, job queue, dead-letter table, or processed-job table.

After Sequin acknowledgment, jobs exist only in memory until processing completes unless a scope has a documented authoritative-source reconciliation path.

If the worker process dies after acknowledgment, queued jobs may be lost unless that scope reconciles its authoritative source.

The current guarantee is therefore:

```text
before acknowledgment:
    retryable delivery

after acknowledgment:
    best-effort in-memory processing
```

The system MUST NOT claim exactly-once or durable at-least-once processing.

This is an explicit MVP trade-off, tracked by #1558, and MUST remain documented until durable worker delivery is introduced. A scope-specific reconciliation loop may repair its own missed wake-ups from authoritative PostgreSQL state, but it does not make the shared in-memory queue durable or change the guarantee for other scopes.

### 12.5 Idempotency and ordering

Handlers MUST tolerate:

* duplicate delivery;
* concurrent delivery;
* replay;
* an older change arriving after a newer one.

Use stable domain identifiers where possible:

```text
product-listing jobs:
    product_listing_events.event_id


search-filter jobs:
    (user_search_filter_id, version, operation)

match jobs:
    (
        user_search_filter_id,
        product_listing_id,
        origin_event_id
    )
```

Projection records SHOULD store the latest applied source version.

An older or equal version MUST NOT overwrite a newer projection state.

Idempotency SHOULD be enforced in the target write through conditional updates, unique constraints, or version checks rather than through in-memory checks.

Current-state invalidation consumers that rebuild output from an authoritative row MUST compare the trigger's source revision with the row's current revision before processing. When they differ, the trigger is stale and MUST be skipped; the consumer MUST NOT evaluate current state while retaining the stale trigger ID. If a current trigger later persists an idempotent row, it MUST recheck and lock that authoritative revision in its final PostgreSQL write transaction through commit, so a stale trigger cannot claim the unique row across external work. Historical notification consumers instead use their exact persisted event or match as the immutable fact and are not invalidated by unrelated later ProductListing events. They require current `ACTIVE` lifecycle while holding a shared ProductListing row lock through notification and delivery-intent commit. ProductListing event decoders and CDC routers MUST reject malformed or unsupported type/group/version/payload contracts; invalid CDC input MUST remain unacknowledged for retry. For ProductListing events, `product_listings.current_event_id` is the current event revision and is separate from the numeric aggregate storage version used for optimistic concurrency. Processed, duplicate, stale, missing-source, withdrawn, and ignored-event outcomes are operationally distinct.

### 12.6 Building projections

A CDC payload may not contain enough information to build a complete projection.

In that case, treat the change as an invalidation signal:

```text
CDC change
    -> extract affected identifier
    -> read current committed PostgreSQL state
    -> build complete projection
    -> conditionally update target
```

Joined or hydrated projections SHOULD reread authoritative state rather than incrementally merging unrelated partial table changes.

Projection mapping belongs to the target adapter.

```text
authoritative query result
    -> adapter-local mapping
    -> search document / key-value item
```

Projection storage types MUST NOT escape their adapter.

### 12.7 Replay and rebuild

Every rebuildable projection MUST document:

* its authoritative source;
* its mapping;
* its source-version strategy;
* how it is rebuilt;
* how live changes are handled during rebuild;
* how the rebuilt projection is verified and activated.

Search indexes SHOULD use versioned indexes and an atomic alias or equivalent cutover.

Existing projections MUST NOT be treated as the recovery source for authoritative data.

### 12.8 Schema evolution

Database migrations affecting CDC consumers MUST use expand-and-contract changes where practical:

1. add new fields;
2. deploy producers;
3. deploy consumers;
4. rebuild or migrate projections;
5. remove old fields.

Tables or columns consumed by CDC MUST NOT be renamed or removed without reviewing:

* Sequin configuration;
* router logic;
* deserialization;
* projection handlers;
* replay and rebuild procedures.

Unknown additive fields SHOULD be tolerated. Missing required fields MUST fail explicitly.

### 12.9 Failure handling

Transient failures SHOULD be retried with bounded backoff.

Structurally invalid or permanently unprocessable changes MUST NOT be silently discarded.

Because the MVP has no durable dead-letter storage, poison changes require operator intervention, code or data correction, and replay.

Logs MUST contain safe identifiers and error categories, not complete source rows, credentials, tokens, or sensitive payloads.

### 12.10 Observability

Monitor at least:

* PostgreSQL replication-slot lag;
* retained WAL growth;
* Sequin delivery lag and retries;
* unacknowledged change age;
* router failures;
* queue depth and saturation;
* handler failures and latency;
* duplicate and stale-version rejections;
* projection freshness;
* projection rebuild status.

Structured logs SHOULD include:

```text
source
table or stream
operation
entity identifier
source version
job type
idempotency key
attempt
outcome
correlation identifier
```

### 12.11 Required tests

CDC tests SHOULD cover:

* insert, update, and delete mapping;
* duplicate delivery;
* concurrent delivery;
* stale changes;
* partial enqueue followed by redelivery;
* queue saturation;
* projection version checks;
* replay;
* full projection rebuild.

The accepted crash-after-ack loss window MUST remain covered by documentation or an operational test.

### 12.12 Non-negotiable rules

1. Every dataset has one operational owner.
2. Only committed PostgreSQL changes are propagated.
3. Projection stores are never part of PostgreSQL transactions.
4. Sequin is acknowledged only after all required jobs are enqueued.
5. All jobs and projection writes are idempotent.
6. Older source versions cannot overwrite newer projections.
7. Projections are rebuildable from authoritative truth.
8. Poison changes are never silently discarded.
9. CDC lag, queue pressure, failures, and freshness are observable.
10. The current post-ack in-memory loss risk is an explicit MVP limitation unless a scope documents an authoritative-source reconciliation path.


## 13. Error boundaries

Each layer owns its errors.

### Core errors

Domain errors describe business rule failures:

```text
Archived
InvalidTransition
TitleTooLong
NotPermittedByPolicy
```

They MUST NOT contain SQLx, HTTP, or external-client errors.

### Service errors

Use-case errors describe outcomes relevant to callers. Variants MUST name the concrete failure a caller can act on. Avoid catch-all policy or failure variants when the real cause is known.

```text
NotFound
AuthenticatedActorRequired
PlanDoesNotAllowAction
ConcurrencyConflict
ProductListingKeyAlreadyExists
ProductListingDetailsQueryFailed
PersistedProductListingAvailabilityInvalid
```

They MAY wrap internal errors privately but MUST expose stable semantic variants. Do not use vague variants such as `Forbidden`, `Conflict`, `InvalidPersistedState`, or `Internal` when a narrower cause is known.

When a service or port error represents an adapter/read-model failure, keep the original cause as `#[source]` with `application::error::BoxError`. Do not convert technical causes to bare unit variants.

### Adapter errors

Adapter errors describe semantic persistence or integration failures without leaking infrastructure types:

```text
ProductListingLookupByIdFailed
ProductListingInsertFailed
ConcurrencyConflict
ProductListingSlugAlreadyExists
InvalidProductListingUrlPersisted
ExternalResponseMissingPrice
```

They MUST NOT expose SQLx, HTTP-client, or SDK error types in public variants. They MUST NOT escape to controllers directly. Use private wrapper types plus `From<..>` implementations when mapping infrastructure errors needs operation context. Preserve the infrastructure error as the semantic error's `#[source]`, usually boxed through `application::error::box_error`. Do not hide adapter error mapping in ad-hoc `map_*_error` helper functions; make the source operation explicit in the wrapper type.

### HTTP mapping

Controllers map service errors to HTTP status and response DTOs.

The service MUST NOT return HTTP status codes.

In `aura-historia-api`, HTTP failures MUST use the crate-local `ApiError` in `error.rs`. It serializes `application/problem+json` with stable public fields such as `status`, `title`, `error`, optional `source`, and optional `detail`. Its fields SHOULD stay private; construct errors through focused constructors and builder methods.

Mappings from transport/service errors to `ApiError` MUST be implemented as `From<ErrorType> for ApiError` in `error.rs` or a unit error-mapping module re-exported by `error.rs`. Do not keep endpoint-local `api_error_from_*` functions once the mapping is reusable for the route unit.

Controllers MAY create `ApiError` directly only for transport-owned validation, such as malformed path parameters, missing headers, or invalid query syntax. Service/use-case errors MUST flow through `From` mappings.

Problem JSON error codes are public API. They MUST be stable, documented by tests, and updated in `docs/swagger.yaml` / `docs/CHANGELOG.md` when behavior changes. In `aura-historia-api`, error codes MUST be declared as dedicated `ApiErrorCode` constants in `error.rs`; controllers and tests MUST use those constants instead of inline string literals such as `"INVALID_UUID"`.

### Logging errors

Avoid logging the same failure at every layer.

- Adapters add technical context to errors.
- Handlers add use-case context.
- The transport or worker boundary records the terminal failure.
- Expected domain errors SHOULD NOT be logged as infrastructure failures.
- Retries SHOULD emit structured events with attempt and delay fields.

---

## 14. Observability and logging

Use structured `tracing` spans and events.

Every externally invoked use case SHOULD have a span:

```rust
#[tracing::instrument(
    name = "get_record_details",
    skip_all,
    fields(
        record_id = %request.record_id,
        principal_type = context.principal.kind(),
        actor_id = tracing::field::Empty,
        request_id = %context.request_id,
        correlation_id = %context.correlation_id,
    )
)]
```

Record the actor identifier only when the principal has one:

```rust
if let Some(actor_id) = context.principal.actor_id() {
    tracing::Span::current()
        .record("actor_id", tracing::field::display(actor_id));
}
```

### Log-Levels

Treat `error!` level as a burning fire and for bugs.
Use info and warn accordingly.
Use debug when sensible.
Less is more as long as everything important is covered.

### Required context

Where available, spans SHOULD include:

```text
use_case
request_id
correlation_id
principal_type
actor_id, when available
aggregate_id
aggregate_version
data_source
result/outcome
retry_attempt
```

### Sensitive data

Logs and traces MUST NOT contain:

- passwords;
- access tokens;
- session tokens;
- authorization headers;
- secret keys;
- complete payment data;
- sensitive personal content;
- raw request/response bodies by default.

Identifiers MAY be logged only according to the project's privacy policy.

### Adapter spans

Adapters SHOULD create child spans for:

```text
postgres.query
postgres.transaction
search.query
key_value.read
external_source.request
```

Record structured fields such as:

```text
operation
table/index/resource
rows_affected
result_count
duration
timeout
status
```

Do not log parameter values that may contain sensitive data.

### Metrics

At minimum, expose metrics for:

- use-case latency and outcomes;
- database query latency and failures;
- transaction conflicts;
- search latency and result counts;
- external-source latency and failures.

### Operational action logging and audit trail

Relevant state-changing and security-sensitive operations MUST produce structured operational information that identifies:

```text
action
outcome
actor_type
actor_id, when authenticated
target type
target identifier
request_id
correlation_id
resulting version or state, when useful
error category, on failure
```

Examples include creation, publication, permission changes, destructive actions, restoration, authentication changes, and administrative overrides.

Success events for authoritative mutations SHOULD be logged after the transaction commits. This avoids recording a successful action that was later rolled back.

Expected validation or authorization failures SHOULD be recorded with an appropriate outcome and severity, without dumping request payloads or secrets.

Business audit records are not ordinary diagnostic logs. Actions requiring durable, queryable, access-controlled, or legally retained history SHOULD also produce an explicit audit record through the project's audit mechanism. Diagnostic logs alone MUST NOT be treated as a compliance-grade audit trail.

## 15. Authentication, authorization, and operation context

### 15.1 Authentication belongs at the transport boundary

REST authentication is performed by middleware or extractors.

The transport layer validates JWTs or other access tokens and maps validated credentials into a transport principal. Service and core code MUST NOT parse tokens, inspect authorization headers, or depend on JWT/framework types.

`aura-historia-api/auth/` owns this boundary for the axum runtime. It accepts Cognito JWTs and Aura Historia access tokens through one authenticator interface. Aura Historia access tokens identify delegated users and map persisted token scopes to `CredentialCapability` values.

Protected endpoints SHOULD use an extractor that guarantees an authenticated principal:

```rust
pub(crate) struct AuthenticatedPrincipal {
    pub user_id: UserId,
}
```

Public endpoints SHOULD use an optional principal extractor:

```rust
pub(crate) enum OptionalPrincipal {
    Anonymous,
    User(UserId),
}
```

Rules for public endpoints:

- A missing token maps to anonymous access.
- A valid token maps to its authenticated principal.
- An invalid, expired, malformed, or revoked token MUST be rejected as an authentication failure.
- Invalid supplied credentials MUST NOT be silently downgraded to anonymous access.

The same principle applies to service credentials and system jobs.

Public axum controllers SHOULD use optional authentication. Missing `Authorization` becomes `Principal::Anonymous`; invalid supplied `Authorization` becomes an HTTP authentication error. This rule is part of the transport contract and MUST be unit-tested for every public endpoint that accepts optional auth.

### 15.2 Service-owned principal model

Controllers map transport principals into service-owned types:

```rust
pub enum Principal {
    Anonymous,
    User(UserId),
    DelegatedUser {
        user_id: UserId,
        capabilities: BTreeSet<CredentialCapability>,
    },
    Service(ServiceId),
    System(SystemActor),
}

pub struct OperationContext {
    pub principal: Principal,
    pub request_id: RequestId,
    pub correlation_id: CorrelationId,
}
```

Framework-specific authentication objects MUST NOT cross the transport boundary.

`aura-historia-api` creates server-side request IDs and accepts correlation IDs only from the configured transport metadata source. Controllers map the transport principal plus request metadata into `OperationContext` before invoking a use case.

Externally invoked use cases SHOULD receive `&OperationContext` separately from their command or query request:

```rust
use_case.execute(&context, command).await
```

The command/request contains business input. The operation context contains caller and request metadata.

This separation prevents trusted identity from being accepted from JSON bodies, query parameters, or path values.

### 15.3 Protected commands

A command that changes protected state MUST require an authenticated actor or an explicitly permitted service/system principal.

Do not model required authentication as `Option<UserId>`:

```rust
// Forbidden for a protected mutation
pub actor_id: Option<UserId>
```

Instead, require the principal at the use-case boundary:

```rust
let actor = context
    .actor_label()
    .ok_or(RenameRecordError::AuthenticatedActorRequired)?;
```

A controller route being protected is not sufficient authorization. The use case MUST enforce authorization or invoke a service/domain policy.

### 15.4 Public queries

A public query such as `GetRecordDetails` MAY accept anonymous callers:

```rust
match &context.principal {
    Principal::Anonymous => {
        // Return public fields only.
    }
    Principal::User(user_id) => {
        // Optionally hydrate user-specific state.
    }
    Principal::Service(service_id) => {
        // Apply the documented service policy.
    }
    Principal::System(actor) => {
        // Apply the documented internal policy.
    }
}
```

Public availability and authenticated personalization are separate concerns.

A public query SHOULD NOT require identity when no authorization, personalization, rate policy, or operational requirement uses it. It MAY still receive `OperationContext` for consistent request correlation and actor-aware logging.

### 15.5 Authorization

Authorization belongs to the use case or an explicit service/domain policy.

Examples:

```text
CanRenameRecord
CanDeleteRecord
CanViewPrivateFields
CanActForWorkspace
```

Authorization MUST use trusted identity from `OperationContext`, never a user identifier supplied by the request body.

Domain methods MAY receive an already-resolved permission or policy value when authorization affects a domain invariant.

### 15.6 Credential scopes

Scopes belong to delegated Aura Historia access tokens, not Cognito JWTs. Cognito-authenticated users, service principals, and system principals use an open-world assumption for credential capability checks; business constraints such as admin role, same-user access, partnership membership, or ownership still MUST be enforced separately by use cases or service policies.

Aura Historia access tokens use a closed-world assumption: a delegated principal has only the scopes stored on the token. Use cases that protect a state change or private read MUST check the narrowest matching `CredentialCapability` before executing protected work.

Scope names SHOULD use `resource:action` with plural resources and coarse, stable actions:

```text
product-listings:write
listing-sources:read
listing-sources:write
partnership-applications:write
users:read
users:write
access-tokens:read
access-tokens:write
search-filters:write
watchlist:write
```

Do not use vague scopes such as `records:manage`. Prefer `read`, `write`, or an explicit non-role action. Roles are not scopes: admin-only use cases MUST check `UserRole::Admin` (or an equivalent service policy) after credential capability checks. Public queries SHOULD NOT require scopes unless authenticated state changes the returned private data or policy.

### 15.7 Operational identity logging

Every externally invoked use case SHOULD have a structured tracing span containing:

```text
principal type
actor identifier, when available
request identifier
correlation identifier
use-case name
target identifier, when available
outcome
```

Anonymous calls MUST be recorded as anonymous, not with a fabricated user identifier.

For relevant mutations such as deletion, publication, permission changes, or administrative actions, the committed success event MUST identify who performed the action:

```rust
tracing::info!(
    event = "record.deleted",
    actor_type = actor.kind(),
    actor_id = %actor.id(),
    record_id = %record_id,
    outcome = "success",
);
```

Access tokens, JWT claims as raw JSON, and authorization headers MUST NOT be logged.

## 16. Configuration and secrets

Configuration is loaded at the composition root.

Adapters receive typed configuration through constructors.

```rust
pub(crate) struct SearchConfig {
    pub endpoint: Url,
}
```

`core` and `service` MUST NOT read environment variables.

Secrets MUST NOT be embedded in domain/application types, logs, errors, or committed configuration files.

External clients and pools SHOULD be constructed once and shared through cloneable handles such as `PgPool` or `Arc<Client>`.

---

## 17. Concurrency and idempotency

### Optimistic concurrency

Aggregate tables SHOULD contain a monotonically increasing version.

Updates MUST include the loaded version:

```sql
UPDATE records
SET
    title = $1,
    version = version + 1
WHERE id = $2
  AND version = $3
RETURNING version
```

No returned row MUST map to an internal concurrency-conflict error when the row was expected to exist. Do not leak the concrete version value in errors. The returned version is authoritative only for PostgreSQL internals and CDC consumers; ordinary use cases SHOULD NOT return it.

A reconciliation that changes aggregates must lock its owner first, then affected aggregate rows in a stable order. Each changed row increments its version, so a stale ordinary aggregate write fails rather than restoring old state.

### Idempotency

Externally retried commands SHOULD accept an idempotency key when duplicate execution would be harmful.

Idempotency handling belongs to the use-case transaction boundary.

The result of a completed idempotent command SHOULD be replayable without repeating its side effects.


## 18. API controller rules

A controller owns:

- REST path/query/header extraction;
- authentication principal extraction;
- mapping transport identity into `OperationContext`;
- request DTOs;
- response DTOs;
- transport-level validation;
- request-to-use-case mapping;
- use-case invocation;
- use-case-result-to-response mapping;
- cache and representation headers owned by the public REST contract;
- service-error-to-HTTP mapping through crate error mappings.

A controller MUST NOT:

- access a database client;
- call a repository;
- build SQL or search DSL;
- compose multiple data sources;
- construct concrete adapters;
- enforce business authorization policy such as admin role, ownership, or partnership relation;
- enforce domain invariants;
- mutate aggregates;
- return storage types;
- decide transaction scope.

Canonical flow:

```text
HTTP request
    -> axum extractor
    -> transport auth principal
    -> OperationContext
    -> REST request DTO/path/query values
    -> service command/request
    -> use-case trait from AppState
    -> service result/view
    -> REST response DTO
    -> HTTP response or ApiError problem JSON
```

`aura-historia-api/state.rs` owns axum state structs. State SHOULD contain inbound use-case trait objects and authenticator trait objects, not repositories. Route modules SHOULD take state through axum `State<T>` and remain thin.

For read endpoints with cache behavior, cache headers are REST contract and belong in the controller. If the cache policy depends on anonymous vs authenticated access, derive it from `OperationContext.principal`, not from raw headers.

Query use cases SHOULD be read-optimized for their public result shape. If an API needs a summary list, the query use case should return summary read models directly instead of returning IDs for controller-side hydration. Controllers MUST NOT introduce N+1 reads to assemble response payloads.

Command use cases SHOULD return the public command result/view directly from their write model. Controllers MUST NOT perform a follow-up read after `create`, `update`, or similar writes to assemble the response.

---

## 19. Async traits and dispatch

Inbound use-case traits are commonly stored as trait objects in use-case bundles:

```rust
Arc<dyn SearchRecordsUseCase>
```

Async trait methods used through `dyn Trait` MUST use the workspace's object-safe async-trait convention, currently `async_trait`.

```rust
#[async_trait::async_trait]
pub trait SearchRecordsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchRecordsRequest,
    ) -> Result<SearchRecordsResult, SearchRecordsError>;
}
```

The attribute MUST also be applied to implementations.

Outbound ports MAY use static dispatch when practical. Consistency is preferred over mixing several async trait styles inside one domain crate.

---

## 20. Testing strategy

Test placement follows visibility and architectural intent.

### 20.1 Tests beside implementation

Tests that need private or `pub(crate)` implementation details MUST live beside the implementation under `#[cfg(test)] mod tests`.

This includes tests for:

- domain internals;
- service handler orchestration with private fakes;
- PostgreSQL rows and mappings;
- private transaction-scoped repositories;
- SQL serialization details;
- adapter-specific request/response mapping;
- real-infrastructure repository and reader behavior.

A test inside a source file MAY use test-only/dev dependencies and MAY start real infrastructure such as PostgreSQL, OpenSearch, or LocalStack.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use test_api::*;

    const POSTGRES: Postgres = Postgres::new("migrations");

    #[aura_integration_test(services = [POSTGRES])]
    async fn should_persist_record() {
        let pool = get_postgres_client().await;
        // Test private adapter implementation directly.
    }
}
```

Running against real infrastructure does not require the test to live in `/tests`.

### 20.2 Black-box tests in `/tests`

A crate-level `tests/` directory is reserved for black-box tests of the crate's deliberate public API.

These tests compile as separate crates and MUST NOT access private or `pub(crate)` items.

They MAY use `dev-dependencies` to wire public service handlers and public adapter factories/readers against real infrastructure:

```text
record-postgres/tests/repository_contract.rs
runtime/tests/record_workflow.rs
api/tests/record_http.rs
```

Do not make an implementation detail public merely to satisfy a `/tests` test. Move that test beside the implementation instead.

### 20.3 Core tests

Core tests MUST verify domain behavior without infrastructure.

Test:

- valid transitions;
- rejected transitions;
- invariants;
- event emission when the aggregate uses domain events;
- idempotent no-op behavior.

Core tests SHOULD normally live in the same file as the tested aggregate or value object.

### 20.4 Service tests

Service handlers MUST be tested with fakes or mocks for their ports.

Test:

- orchestration;
- transaction begin/commit behavior;
- use of the same transaction across several factories;
- batching;
- fallback behavior;
- optional enrichment;
- preservation of search order;
- error translation;
- authorization decisions;
- skipping persistence when no domain state changed.

These tests SHOULD normally live in the same use-case file under `#[cfg(test)]` so handler internals and private fakes do not need public visibility.

### 20.5 PostgreSQL adapter tests

PostgreSQL repositories and readers SHOULD have real PostgreSQL tests beside their implementation.

Test:

- `FromRow` mappings;
- aggregate rehydration;
- insert/update semantics;
- optimistic concurrency;
- rollback behavior;
- cross-entity transactions;
- joined readers;
- migration compatibility.

Rows, mapping helpers, and scoped repository types MUST remain non-public.

### 20.6 Other adapter tests

Each adapter SHOULD test:

- request serialization;
- response deserialization;
- mapping to application types;
- timeout/error mapping;
- stale-version handling where applicable.

Private adapter tests belong beside the implementation. Public contract tests MAY live in `/tests`.

### 20.7 Controller tests

Controller tests SHOULD verify:

- DTO deserialization;
- DTO/use-case mapping;
- status-code mapping;
- response serialization;
- authentication/context mapping;
- missing-token behavior on public routes;
- invalid-token rejection on public and protected routes;
- protected-route authentication enforcement.

They SHOULD mock only the inbound use-case trait, not repositories.

For axum API controllers, tests SHOULD exercise the `Router` with fake inbound use-case traits and fake authenticators. Cover success, request validation, auth rejection, service-error mapping, response DTO shape, and contract headers.

### 20.8 Acceptance tests

Acceptance tests SHOULD:

- verify the most important behavior from outside the system;
- work only against the exposed REST API;
- be written as theses about system behavior;
- live in the API/runtime black-box `tests/` suite.

For `aura-historia-api`, black-box API tests SHOULD use `test-api::AuraHistoriaApi` as a process-lived test service. Declare the API once near the top of the test file and pass `&AURA_API` to `#[aura_integration_test]`; do not start and stop the HTTP server inside each test body.

```rust
const POSTGRES: test_api::Postgres = test_api::Postgres::new("migrations");
static AURA_API: test_api::AuraHistoriaApi = test_api::AuraHistoriaApi::new(aura_api_app);

#[test_api::aura_integration_test(services = [POSTGRES, &AURA_API])]
async fn should_get_listing_source_by_id_with_aura_access_token() {
    let response = match reqwest::Client::new()
        .get(format!(
            "{}/api/v1/admin/listing-sources/{listing_source_id}",
            AURA_API.base_url(),
        ))
        .bearer_auth(token)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => panic!("failed to call API: {error}"),
    };
}
```

Acceptance tests for authenticated routes SHOULD use Aura Historia access tokens when the public contract supports them. Seed credentials through the same storage adapter used by the runtime, then call the real HTTP endpoint with a bearer token. Keep one source module per API unit, but compatible modules SHOULD be declared by a small number of explicit integration-suite binaries so process-lived real infrastructure is amortized. Mutable application and projection data MUST reset between tests.

## 21. Naming conventions

Use these suffixes consistently:

| Suffix | Meaning |
|---|---|
| `...Command` | Write use-case input |
| `...Request` | Read use-case input |
| `...Result` | Write result or paginated query result |
| `...View` | Final or partial application read model |
| `...Summary` | Compact application read model |
| `...UseCase` | Controller-facing inbound trait |
| `...Handler` | Focused use-case implementation |
| `...Repository` | Aggregate reconstruction/persistence |
| `...Reader` | Purpose-specific read capability |
| `...Policy` | Domain or application decision abstraction |
| `...Row` | PostgreSQL row representation |
| `...Document` | Search document representation |

| `...Record` | External/graph/source response representation |
| `...Dto` | Transport representation |
| `...Event` | Domain event |
| `-core` | Entity domain crate |
| `-service` | Entity application/use-case crate |
| `-postgres` | Entity PostgreSQL adapter crate |
| `-opensearch` | Entity OpenSearch adapter crate |


Avoid vague names:

```text
Manager
Helper
Util
Common
GenericRepository
DataService
DatabaseService
OpenSearchQuery
PostgresPort
```

Names SHOULD describe business intent or application capability. When bounded contexts have similarly shaped lifecycle values, keep their Rust names distinct (`SearchFilterState`, `WatchlistState`) instead of using a generic compatibility alias.

---

## 22. Forbidden patterns

The following patterns MUST NOT be introduced without an approved architecture change:

### Generic cross-store repository

```rust
trait Repository<T, Id> {
    async fn save(&self, value: T);
    async fn find(&self, id: Id);
    async fn delete(&self, id: Id);
}
```

### Repository used for presentation reads

```rust
record_repository.search_with_container_and_user_state(...)
```

### Storage type escaping adapter

```rust
fn controller(...) -> Json<RecordRow>
```

### Controller orchestration

```rust
let hits = search_client.search(...).await?;
let states = postgres_reader.read(...).await?;
let result = merge(hits, states);
```

### N+1 hydration

```rust
for hit in hits {
    user_state_reader.find_one(actor_id, hit.id).await?;
}
```

### Domain depending on infrastructure

```rust
#[derive(sqlx::FromRow)]
pub struct Record { ... }
```

### One god service

```rust
struct RecordService {
    repository: ...,
    search: ...,
    metadata: ...,
    user_state: ...,
    queue: ...,
    cache: ...,
    // dependencies for every use case
}
```

### Hidden distributed transaction

```text
BEGIN PostgreSQL
update PostgreSQL
update search index
update key-value store
COMMIT PostgreSQL
```

### Logging sensitive payloads

```rust
tracing::info!(?request_body, authorization = %header);
```

### Silent persisted-state corruption

```rust
impl From<RecordRow> for Record {
    // unchecked construction that bypasses invariants
}
```

---

## 23. Implementation-agent checklist

Agents MUST read this document before implementing architecture-affecting work.

### Before coding

- [ ] Identify the bounded context and aggregate.
- [ ] Identify whether the change is a command, query, projection, or infrastructure concern.
- [ ] Place the aggregate/value objects in the correct `<entity>-core` crate.
- [ ] Place use-case contracts and handler implementation in one `<entity>-service/src/use_cases/...` file.
- [ ] Place each infrastructure implementation in its `<entity>-<adapter>` crate.
- [ ] Define or reuse the smallest capability-oriented outbound ports.
- [ ] Confirm that no port is named after a database.
- [ ] Decide whether the final read model belongs to the use case.
- [ ] Decide whether the write requires one PostgreSQL transaction.
- [ ] Identify invariant-critical reads that must use the same transaction.
- [ ] Identify optional additional-source enrichment and its failure behavior.
- [ ] Define error translation at each layer.
- [ ] Define mapping location for every boundary type.

### While coding

- [ ] Keep domain fields private.
- [ ] Keep adapter rows/documents/items private.
- [ ] Use `FromRow` for PostgreSQL row deserialization.
- [ ] Use `TryFrom` where mapping can fail.
- [ ] Bind domain values to SQL inside the DAO/repository implementation.
- [ ] Avoid public domain-to-row conversions for writes.
- [ ] Batch hydration queries.
- [ ] Preserve source ordering after hydration.
- [ ] Keep handlers free of SQLx and concrete adapter imports.
- [ ] Use an abstract `UnitOfWork` and transaction-scoped repository/operational reader factories.
- [ ] Use chained temporary repositories for one-off transactional calls.
- [ ] Explicitly commit successful SQLx transactions.
- [ ] Add a use-case tracing span with safe structured fields.
- [ ] Map transport authentication into service-owned `OperationContext`.
- [ ] Ensure protected mutations reject anonymous principals.
- [ ] Log relevant committed actions with actor, target, and outcome.
- [ ] Do not leak adapter errors or types across boundaries.
- [ ] Use the narrowest possible visibility.

### Before completion

- [ ] Add private/internal tests beside the implementation under `#[cfg(test)]`.
- [ ] Keep `/tests` for black-box public-API tests only.
- [ ] Do not widen visibility solely for tests.
- [ ] Add domain unit tests.
- [ ] Add use-case orchestration tests.
- [ ] Add adapter mapping/integration tests.
- [ ] Add transaction/concurrency tests where relevant.
- [ ] Add controller DTO/error mapping tests.
- [ ] Add acceptance tests
- [ ] Verify no N+1 access pattern was introduced.
- [ ] Verify Cargo dependencies enforce core <- service <- adapters <- runtime/transport.
- [ ] Verify canonical crates use durable shared owners directly.
- [ ] Verify no service/core import points toward an adapter.
- [ ] Verify no controller accesses a repository or database client.
- [ ] Verify logs contain no secrets or sensitive payloads.
- [ ] Update this document when a new general architectural rule was introduced.

---

## 24. Pull-request review checklist

Reviewers SHOULD reject changes that cannot answer these questions clearly:

1. Which layer owns each new type?
2. Is each public type intentionally public?
3. Does the use case express business intent?
4. Does the handler depend only on required capabilities?
5. Is aggregate persistence separated from read-model construction?
6. Are storage mappings confined to adapters?
7. Does the transaction contain exactly the invariant-critical PostgreSQL work?
8. Are cross-source reads composed in a handler rather than a controller?
9. Is trusted caller identity carried through `OperationContext` rather than request input?
10. Do relevant mutations record actor, target, and committed outcome safely?
11. Are errors and logs safe and appropriately translated?
12. Are public items required by a real production crate boundary rather than only by tests?
13. Are private/real-infrastructure implementation tests beside the code and `/tests` limited to black-box behavior?
14. Are the important rules covered by tests?

---

## 25. Informative references

These references explain library behavior used by the conventions above:

- SQLx `FromRow`: <https://docs.rs/sqlx/latest/sqlx/trait.FromRow.html>
- SQLx `Transaction`: <https://docs.rs/sqlx/latest/sqlx/struct.Transaction.html>
- SQLx `query_as`: <https://docs.rs/sqlx/latest/sqlx/fn.query_as.html>
- `async-trait`: <https://docs.rs/async-trait/latest/async_trait/>
- `tracing`: <https://docs.rs/tracing/latest/tracing/>

The rules in this document remain authoritative even when a referenced implementation library changes. Library upgrades MUST be reviewed for effects on these conventions.

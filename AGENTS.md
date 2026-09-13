**Think caveman. Talk caveman. Few word.**

---

# DOX

## Purpose

- Own repo DOX rail.
- Own root files: `Cargo.toml`, `Cargo.lock`, `README.md`, `LICENSE`, `.cargo/`, `depgraph-rules.toml`, repo config.

## Core Design

- Repo be Rust workspace for AWS serverless backend.
- `src/` hold crates. `migrations/` hold Postgres business schema. `infra/` shape cloud. `docs/` hold public contract. `mjml/` and `opensearch/` hold shared assets.
- Domain crates keep rules. API and Lambda crates stay thin around transport and runtime glue.

## Ownership

- This file rule whole repo.
- Child doc rule deeper path.
- Near doc win detail. Child no break parent.

## Local Contracts

- Read root, then each `AGENTS.md` on path, before edit.
- Re-read in same session. No trust memory.
- After meaningful change, do DOX pass.
- Update nearest doc when purpose, shape, workflow, contract, input, output, limit, side effect, or user pref change.
- Refresh child index. Kill stale words.
- Put durable user prefs here or nearest child doc.
- Persist enum values in `SCREAMING_SNAKE_CASE`; keep standardized identifier formats such as ISO language codes canonical.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- Less is more.
- Keep docs short, clear, current.
- In `src`, make doc by crate. No module doc unless module become crate boundary.

## Architecture Law

- `docs/arch.md` be design source.
- Before architecture or code edit, read matching `docs/arch.md` sections. No memory.
- If task touches Rust backend shape, load matching project skill from `.agents/skills/`.
- If task bends `docs/arch.md`, say why. Update doc when new general rule.

## Skill Routing

- Enum creation/change, canonical string identity, persisted enum mapping → `aura-rust-enum`.
- Use case, service flow, command, query, port, service error → `aura-rust-use-case`.
- Aggregate persistence, repository, PostgreSQL rows/mapping/version → `aura-rust-repository`.
- Reader, read model, joined read, hydration, search/user-state read → `aura-rust-reader`.
- API route, axum controller, DTO, auth extractor, `OperationContext`, `ApiError` → `aura-rust-api-endpoint`.
- Transaction, `UnitOfWork`, multi-repo write, idempotency, cross-datasource boundary → `aura-rust-transactional-flow`.
- CDC, Sequin, projection job, OpenSearch/key-value projection, replay/rebuild → `aura-rust-projection`.
- Test placement, fakes, real infra tests, validation commands → `aura-rust-test`.
- Before final answer on meaningful backend code change or review → `aura-rust-review-architecture`.

## Backend Hard Rules

- Domain no depend on infra.
- API and Lambda stay thin.
- Service owns use cases.
- Service owns transactions.
- Repositories persist aggregates.
- Readers build read models.
- No generic cross-store repository.
- No repository for presentation reads.
- No storage row, document, item, or DTO escape adapter.
- No controller orchestration.
- No N+1 hydration.
- No hidden distributed transaction.
- No sensitive payload logging.
- No silent persisted-state corruption.

## Arch Map

- Layout and dependency direction: `docs/arch.md` §3.
- DDD and type ownership: §4-5.
- Use cases and ports: §6-7.
- Repositories and readers: §8-9.
- Mapping and serialization: §10.
- Transactions: §11.
- CDC and projections: §12.
- Errors, logging, auth, config: §13-16.
- Concurrency and idempotency: §17.
- Controllers: §18.
- Testing: §20.
- Naming, forbidden patterns, checklists: §21-24.

## Verification

- Rust all: `cargo check --workspace`
- Rust dep graph: `cargo depgraph-check check`
- Rust tests: `cargo test --workspace --lib --all-features`
- Infra test: `npm --prefix infra test`
- Infra synth: `npm --prefix infra run synth:all`

## Child DOX Index

- `.agents/AGENTS.md` — project-local agent skills.
- `.github/AGENTS.md` — GitHub flow.
- `docs/AGENTS.md` — public docs.
- `deploy/AGENTS.md` — hybrid release/controller/platform tooling; default-off during integration.
- `infra/AGENTS.md` — CDK infra.
- `mjml/AGENTS.md` — email templates.
- `opensearch/AGENTS.md` — shared OpenSearch assets.
- `src/AGENTS.md` — Rust crates and `src/opensearch/`.

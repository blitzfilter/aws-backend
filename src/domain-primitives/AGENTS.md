# DOX

## Purpose

- Own domain-neutral primitives and newtype macros.

## Core Design

- Own `ChangeOutcome`, generic events, version wrappers (Serde-capable or internal `no_serde`), strict UUIDv7 TypeID object-ID support, reusable UUID/string newtypes, slug IDs/macros, and generic query values.
- Object-ID macro owns canonical text/serde, typed parse errors, raw UUID validation, and hidden dependency paths. Entity crates own concrete IDs and registered prefixes.
- No entity IDs except legacy `EventId`, business rules, transport, persistence, SDKs, or runtime config.
- `test-data` is explicit. Object-ID `Dummy<Faker>` uses hidden reexports and the supplied RNG for UUIDv7 random bits; legacy macro callers still need matching feature/dependency paths.

## Ownership

- This doc rule `src/domain-primitives/**`.
- Parent doc: `src/AGENTS.md`.

## Verification

- `cargo check -p domain-primitives --all-targets --all-features`
- `cargo test -p domain-primitives --all-features`
- `cargo clippy -p domain-primitives --all-targets --all-features -- -D warnings`

# DOX

## Purpose

- Own `fxrate-core` crate.
- Own `FxRateId`, immutable EUR-base FX snapshots, and exact money conversion.

## Core Design

- Domain-only. `FxRateId` is a strict UUIDv7-backed `fx_` object ID. Quotes are `units_per_eur` scaled by `FX_RATE_SCALE`.
- Snapshots contain every supported currency, including EUR at the exact scale.
- Conversion uses `money` values, checked integer arithmetic, and source/target minor-unit exponents.

## Ownership

- Parent doc: `src/AGENTS.md`.

## Work Guidance

- Think caveman. Talk caveman. Few word.
- No provider, SQL, transport, float, or decimal-string conversion here.

## Verification

- `cargo check -p fxrate-core`
- `cargo test -p fxrate-core --all-features`

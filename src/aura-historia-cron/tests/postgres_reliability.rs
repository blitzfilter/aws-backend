//! Local-only black-box proof: shipped preflight binary; public runtime with synthetic PG work.
//! No private production imports, migration adoption, cloud services, or business-job acceptance.
#[path = "postgres_reliability/custody.rs"]
mod custody;
#[path = "postgres_reliability/fixture.rs"]
mod fixture;
#[path = "postgres_reliability/preflight.rs"]
mod preflight;

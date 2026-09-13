use super::*;
use sqlx::{
    SqlSafeStr,
    migrate::{Migration, MigrationType},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn should_accept_only_shipped_transactional_sources() -> TestResult {
    assert_eq!(source(Target::Business)?.iter().count(), 1);
    assert_eq!(source(Target::Crawler)?.iter().count(), 6);
    let workspace = include_str!("../../../../../Cargo.toml");
    assert!(workspace.contains("sqlx = { version = \"=0.9.0\""));
    Ok(())
}

#[test]
fn should_refuse_nontransactional_down_changed_and_incremental_sources() {
    for (kind, no_tx, version) in [
        (MigrationType::Simple, true, 1),
        (MigrationType::ReversibleDown, false, 1),
        (MigrationType::ReversibleUp, false, 1),
        (MigrationType::Simple, false, 2),
    ] {
        let source = Migrator::with_migrations(vec![Migration::new(
            version,
            "private fixture".into(),
            kind,
            "SELECT 1".into_sql_str(),
            no_tx,
        )]);
        assert!(matches!(
            guard_source(&source, &[1]),
            Err(Failure {
                code: Code::UnsupportedSource,
                ..
            })
        ));
    }
    let mut source = Migrator::with_migrations(vec![]);
    assert!(guard_source(&source, &[1]).is_err());
    source.locking = false;
    assert!(guard_source(&source, &[]).is_err());
    source.locking = true;
    source.no_tx = true;
    assert!(guard_source(&source, &[]).is_err());
    source.no_tx = false;
    source.ignore_missing = true;
    assert!(guard_source(&source, &[]).is_err());
}

#[test]
fn should_allow_only_target_prerequisite_extensions_in_expected_schemas() {
    for target in [Target::Business, Target::Crawler] {
        for name in [
            "plpgsql",
            "pg_trgm",
            "unaccent",
            "pg_ttl_index",
            "pgcrypto",
            "unknown",
        ] {
            for schema in ["pg_catalog", "public", "unknown"] {
                let expected = (name == "plpgsql" && schema == "pg_catalog")
                    || (schema == "public"
                        && match target {
                            Target::Business => {
                                matches!(name, "pg_trgm" | "unaccent" | "pg_ttl_index")
                            }
                            Target::Crawler => name == "pgcrypto",
                        });
                let extensions = [Extension {
                    name: name.into(),
                    schema: schema.into(),
                }];
                assert_eq!(extensions_allowed(target, &extensions), expected);
            }
        }
    }
}

#[test]
fn should_require_exact_preloaded_ttl_library() {
    for setting in [
        "pg_ttl_index",
        "other, pg_ttl_index",
        "\"pg_ttl_index\"",
        "$libdir/pg_ttl_index",
    ] {
        assert!(ttl_preloaded(setting));
    }
    for setting in [
        "",
        "other",
        "not_pg_ttl_index",
        "pg_ttl_index_extra",
        "secret/pg_ttl_index",
    ] {
        assert!(!ttl_preloaded(setting));
    }
}

#[test]
fn should_reject_untrusted_catalog_identifiers() {
    for name in ["pg_class", "pg_largeobject_metadata", "pg_subscription"] {
        assert!(catalog_identifier(name));
    }
    for name in [
        "pg_class\"; SELECT 1;--",
        "public",
        "PG_CLASS",
        "pg_1",
        "pg_class.secret",
    ] {
        assert!(!catalog_identifier(name));
    }
}

#[test]
fn should_refuse_every_object_not_in_the_extension_internal_allowance() {
    let allowed = BTreeSet::from([(1, 16384), (2, 16385)]);
    assert!(objects_allowed(1, &[], &allowed));
    assert!(objects_allowed(1, &[16384], &allowed));
    assert!(objects_allowed(2, &[16385], &allowed));
    assert!(!objects_allowed(2, &[16384], &allowed));
    assert!(!objects_allowed(1, &[16384, 16386], &allowed));
    assert!(!objects_allowed(
        1,
        &vec![16384; OBJECT_LIMIT as usize + 1],
        &allowed
    ));
    // Same refusal regardless of which catalog represents the unknown object.
    for class in 1..=128 {
        assert!(!objects_allowed(class, &[20000], &allowed));
    }
}

#[test]
fn should_refuse_existing_ledger_schema_large_object_or_subscription_without_reading_history() {
    assert!(!FreshnessPrelude::default().allows_initialization());
    assert!(
        FreshnessPrelude {
            public_schema_exists: true,
            ..Default::default()
        }
        .allows_initialization()
    );
    for prelude in [
        FreshnessPrelude {
            public_schema_exists: true,
            ledger_exists: true,
            ..Default::default()
        },
        FreshnessPrelude {
            public_schema_exists: true,
            unknown_schema_exists: true,
            ..Default::default()
        },
        FreshnessPrelude {
            public_schema_exists: true,
            large_object_exists: true,
            ..Default::default()
        },
        FreshnessPrelude {
            public_schema_exists: true,
            subscription_exists: true,
            ..Default::default()
        },
    ] {
        assert!(!prelude.allows_initialization());
    }
    // No ledger contents/row count enter this decision: empty and exact are equally refused.
}

// Real PostgreSQL seam (intentionally not a public test API or a fake success test):
// The supervised server_preflight_postgres fresh_bootstrap CLI fixture owns the
// pinned TTL positive and all 14 negatives, with catalog snapshots and parent-verified cleanup.
// ttl_tests.rs retains policy unit tests only. Further owned, opt-in tests can use fixture-only
// config. No ambient URLs, Docker pulls, cloud credentials or remote Docker endpoints.
// Required proof: stock fresh crawler + business/preloaded TTL success and exact public
// gates; repeat/empty/exact/dirty/mismatched ledger refusal with unchanged snapshots;
// unknown empty schema, table/view/sequence/type/function/index/trigger/event trigger,
// publication/database-owned subscription/large object (including low OID), unknown or misplaced extension, auto-dependent
// object on a prerequisite all refuse. Exercise genuine extension internal objects.
// Two initializer sessions plus a SQLx-only contender must recheck under retained locks;
// arbitrary writers remain excluded by operator custody, not by advisory-lock claims.
// Inject permission/statement/lock/connection/commit/verification/close failures before
// and after writes; assert no retry/repair and UNKNOWN_OUTCOME after possible writes.
// Finally inspect pg_stat_activity/pg_locks and histories after process termination.
// Unit/loopback tests alone do not prove catalog, transaction, TLS or cleanup semantics.
// Run real PostgreSQL proof through the supervising parent, never its internal child directly.

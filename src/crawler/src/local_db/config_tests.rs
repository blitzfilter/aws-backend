use super::*;
use crate::local_db::{
    DEMO_DB_NAME, DEMO_SCRAPER_DB_NAME, DEMO_SPIDER_DB_NAME, SERVER_DB_NAME, assert_redacted_chain,
    demo_db_url, demo_scraper_db_url, demo_spider_db_url, server_db_url,
};
use platform_postgres::PostgresConnectError;
use rstest::rstest;
use sqlx::postgres::PgSslMode;
use std::collections::HashMap;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CA_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/crawler-postgres-ca.pem"
);
const CRAWLER_URL: &str =
    "postgres://private-user:private-password@crawler.invalid:6543/crawler_state";
const BUSINESS_URL: &str =
    "postgres://private-user:private-password@business.invalid:6432/business_data";

fn inputs(stage: &str) -> HashMap<&'static str, String> {
    let mut inputs: HashMap<_, _> = [
        ("STAGE", stage),
        ("POSTGRES_SSL_MODE", "disable"),
        ("LOCAL_DB_URL", CRAWLER_URL),
        ("BUSINESS_DATABASE_URL", BUSINESS_URL),
    ]
    .into_iter()
    .map(|(key, value)| (key, value.to_owned()))
    .collect();
    if matches!(stage, "dev" | "prod") {
        inputs.insert("POSTGRES_SSL_MODE", "verify-full".into());
        inputs.insert("POSTGRES_SSL_ROOT_CERT", CA_FILE.into());
    }
    inputs
}

fn server(
    inputs: &HashMap<&'static str, String>,
) -> Result<ServerDatabaseConfig, PostgresPoolConfigError> {
    parse_postgres_environment(
        |key| inputs.get(key).cloned().ok_or(VarError::NotPresent),
        |get| ServerDatabaseConfig::from_lookup(16, 8, get),
    )
}

#[rstest]
#[case("dev")]
#[case("prod")]
fn should_validate_both_remote_urls_and_retain_caps_without_bootstrap(
    #[case] stage: &str,
) -> TestResult {
    let inputs = inputs(stage);
    let config = server(&inputs)?;
    assert_eq!(config.crawler.host(), "crawler.invalid");
    assert_eq!(config.crawler.port(), 6543);
    assert_eq!(config.crawler.database(), "crawler_state");
    assert_eq!(config.crawler.max_connections(), 16);
    assert_eq!(config.business.host(), "business.invalid");
    assert_eq!(config.business.port(), 6432);
    assert_eq!(config.business.database(), "business_data");
    assert_eq!(config.business.max_connections(), 8);
    for pool in [&config.crawler, &config.business] {
        assert!(matches!(
            pool.connect_options().get_ssl_mode(),
            PgSslMode::VerifyFull
        ));
        assert_eq!(
            pool.connect_options().get_application_name(),
            Some("crawler-server")
        );
    }
    assert!(!format!("{config:?}").contains("private-"));
    assert!(matches!(
        LocalDevelopmentConfig::from_lookup("crawler-test", |key| inputs.get(key).cloned()),
        Err(LocalDatabaseError::LocalStageRequired)
    ));
    Ok(())
}

#[rstest]
#[case("STAGE")]
#[case("POSTGRES_SSL_MODE")]
#[case("LOCAL_DB_URL")]
#[case("BUSINESS_DATABASE_URL")]
fn should_reject_missing_server_input_without_local_fallback(#[case] missing: &'static str) {
    let mut inputs = inputs("local");
    inputs.remove(missing);
    assert!(
        matches!(server(&inputs), Err(PostgresPoolConfigError::MissingInput(key)) if key == missing)
    );
}

#[rstest]
#[case("dev")]
#[case("prod")]
fn should_reject_real_stage_without_verified_tls_and_ca(#[case] stage: &str) {
    let mut inputs = inputs(stage);
    inputs.remove("POSTGRES_SSL_ROOT_CERT");
    assert!(matches!(
        server(&inputs),
        Err(PostgresPoolConfigError::RootCertificateRequired)
    ));
    inputs.insert("POSTGRES_SSL_MODE", "disable".into());
    assert!(matches!(
        server(&inputs),
        Err(PostgresPoolConfigError::VerifyFullRequired)
    ));
}

#[rstest]
#[case(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/missing-private-ca.pem"))]
#[case(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))]
#[case(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))]
fn should_reject_unreadable_invalid_or_non_file_ca_without_printing_path(#[case] path: &str) {
    let mut inputs = inputs("prod");
    inputs.insert("POSTGRES_SSL_ROOT_CERT", path.into());
    let result = server(&inputs);
    assert!(result.is_err());
    let diagnostic = format!("{result:?}");
    assert!(!diagnostic.contains(path));
    assert!(!diagnostic.contains("private-"));
}

#[rstest]
fn should_reject_url_downgrades_overrides_and_malformed_urls(
    #[values("LOCAL_DB_URL", "BUSINESS_DATABASE_URL")] key: &'static str,
    #[values(
        "?sslmode=disable",
        "?sslmode=prefer",
        "?sslmode=require",
        "?sslmode=verify-ca",
        "?sslmode=verify-full&sslmode=verify-full",
        "?sslrootcert=private-ca",
        "?host=localhost",
        "?options=private-options",
        "?application_name=another-app",
        "#private-fragment"
    )]
    suffix: &str,
) {
    let mut inputs = inputs("prod");
    inputs.insert(key, format!("{CRAWLER_URL}{suffix}"));
    let result = server(&inputs);
    assert!(result.is_err());
    assert!(!format!("{result:?}").contains("private-"));
}

#[rstest]
fn should_reject_empty_or_invalid_url_in_either_pool(
    #[values("LOCAL_DB_URL", "BUSINESS_DATABASE_URL")] key: &'static str,
    #[values("", "not-a-private-url", "postgres://localhost/crawler_state")] url: &str,
) {
    let mut inputs = inputs("local");
    inputs.insert(key, url.into());
    assert!(server(&inputs).is_err());
}

#[rstest]
#[case("local")]
#[case("ephemeral")]
#[case("test")]
fn should_allow_explicit_non_real_server_and_demo_config(#[case] stage: &str) -> TestResult {
    let inputs = inputs(stage);
    let config = server(&inputs)?;
    for pool in [&config.crawler, &config.business] {
        assert!(matches!(
            pool.connect_options().get_ssl_mode(),
            PgSslMode::Disable
        ));
    }
    let local =
        LocalDevelopmentConfig::from_lookup("crawler-test", |key| inputs.get(key).cloned())?;
    for (name, url) in [
        (SERVER_DB_NAME, server_db_url(&local)),
        (DEMO_DB_NAME, demo_db_url(&local)),
        (DEMO_SCRAPER_DB_NAME, demo_scraper_db_url(&local)),
        (DEMO_SPIDER_DB_NAME, demo_spider_db_url(&local)),
    ] {
        let pool = local.pool_config(name, 5)?;
        assert_eq!(pool.host(), "localhost");
        assert_eq!(pool.database(), name);
        assert_eq!(pool.max_connections(), 5);
        assert!(url.ends_with(name));
        assert_eq!(
            pool.connect_options().get_application_name(),
            Some("crawler-test")
        );
    }
    Ok(())
}

#[rstest]
#[case(None)]
#[case(Some(""))]
#[case(Some("dev"))]
#[case(Some("prod"))]
#[case(Some("LOCAL"))]
#[case(Some("unknown"))]
fn should_reject_bootstrap_and_demos_without_explicit_non_real_stage(#[case] stage: Option<&str>) {
    let mut inputs = inputs("local");
    inputs.remove("STAGE");
    if let Some(stage) = stage {
        inputs.insert("STAGE", stage.into());
    }
    assert!(
        LocalDevelopmentConfig::from_lookup("crawler-test", |key| inputs.get(key).cloned())
            .is_err()
    );
}

#[rstest]
#[case(None)]
#[case(Some("prefer"))]
fn should_require_explicit_shared_tls_mode_for_local_commands(#[case] mode: Option<&str>) {
    let mut inputs = inputs("local");
    inputs.remove("POSTGRES_SSL_MODE");
    if let Some(mode) = mode {
        inputs.insert("POSTGRES_SSL_MODE", mode.into());
    }
    assert!(
        LocalDevelopmentConfig::from_lookup("crawler-test", |key| inputs.get(key).cloned())
            .is_err()
    );
}

#[rstest]
#[case("PGSSLCERT")]
#[case("PGSSLKEY")]
#[case("PGSSLROOTCERT")]
#[case("PGOPTIONS")]
fn should_reject_ambient_tls_and_options_for_server_and_local_commands(#[case] key: &'static str) {
    let mut inputs = inputs("local");
    inputs.insert(key, "private-ambient-value".into());
    assert!(
        matches!(server(&inputs), Err(PostgresPoolConfigError::UnsupportedAmbientSetting(found)) if key == found)
    );
    assert!(
        LocalDevelopmentConfig::from_lookup("crawler-test", |key| inputs.get(key).cloned())
            .is_err()
    );
}

#[test]
fn should_reject_zero_pool_cap_before_connecting() {
    let inputs = inputs("local");
    for (crawler, business) in [(0, 8), (16, 0)] {
        assert!(matches!(
            ServerDatabaseConfig::from_lookup(crawler, business, |key| inputs.get(key).cloned()),
            Err(PostgresPoolConfigError::ZeroMaxConnections)
        ));
    }
}

#[test]
fn should_redact_local_connection_and_migration_errors_but_retain_sources() {
    let errors = [
        LocalDatabaseError::Connect(PostgresConnectError::from(sqlx::Error::Protocol(
            CRAWLER_URL.into(),
        ))),
        LocalDatabaseError::DatabaseCreation(PostgresConnectError::from(sqlx::Error::Protocol(
            CRAWLER_URL.into(),
        ))),
        LocalDatabaseError::DatabaseCreation(PostgresConnectError::from(
            sqlx::Error::Configuration(application::error::box_error(std::io::Error::other(
                CRAWLER_URL,
            ))),
        )),
        LocalDatabaseError::from(sqlx::migrate::MigrateError::Execute(sqlx::Error::Protocol(
            CRAWLER_URL.into(),
        ))),
    ];
    for error in errors {
        assert!(assert_redacted_chain(&error, &[CRAWLER_URL, "private-"]) >= 2);
    }
}

#[test]
fn should_keep_server_source_connect_only_with_validation_before_cloudwatch() -> TestResult {
    let source = include_str!("../bin/server.rs");
    for forbidden in [
        "bootstrap_local_database(",
        "bootstrap_all_local_databases(",
        "start_local_postgres(",
        "sqlx::migrate!",
        "PgPoolOptions",
    ] {
        assert!(
            !source.contains(forbidden),
            "server must not bootstrap or bypass shared pools"
        );
    }
    let config = source
        .find("ServerDatabaseConfig::from_lookup")
        .ok_or("server must validate both database configurations")?;
    let aws = source
        .find("aws_config::defaults")
        .ok_or("CloudWatch support must remain")?;
    assert!(config < aws);
    Ok(())
}

#[test]
fn should_preserve_unicode_and_absent_environment_values() -> TestResult {
    let value = "private-pässword and private/证书.pem";
    parse_postgres_environment(
        |key| match key {
            "LOCAL_DB_URL" => Ok(value.to_owned()),
            _ => Err(VarError::NotPresent),
        },
        |get| {
            assert_eq!(get("LOCAL_DB_URL").as_deref(), Some(value));
            assert_eq!(get("POSTGRES_SSL_ROOT_CERT"), None);
            Ok::<_, PostgresPoolConfigError>(())
        },
    )?;
    Ok(())
}

#[cfg(unix)]
fn invalid_environment_value(bytes: &[u8]) -> Result<String, VarError> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    OsString::from_vec(bytes.to_vec())
        .into_string()
        .map_err(VarError::NotUnicode)
}

#[cfg(unix)]
#[rstest]
#[case(
    "LOCAL_DB_URL",
    b"postgres://private-user:private-\xff-password@crawler.invalid/crawler_state"
)]
#[case(
    "BUSINESS_DATABASE_URL",
    b"postgres://private-user:private-\xff-password@business.invalid/business_data"
)]
#[case("POSTGRES_SSL_ROOT_CERT", b"/private-\xff-ca.pem")]
#[case("STAGE", b"private-\xff-stage")]
#[case("POSTGRES_SSL_MODE", b"private-\xff-mode")]
#[case("PGSSLCERT", b"/private-\xff-cert.pem")]
#[case("PGSSLKEY", b"/private-\xff-key.pem")]
#[case("PGSSLROOTCERT", b"/private-\xff-ca.pem")]
#[case("PGOPTIONS", b"private-\xff-options")]
fn should_reject_non_unicode_server_environment_without_replacement_or_secret_errors(
    #[case] invalid_key: &'static str,
    #[case] bytes: &[u8],
) -> TestResult {
    let inputs = inputs("prod");
    let error = parse_postgres_environment(
        |key| {
            if key == invalid_key {
                invalid_environment_value(bytes)
            } else {
                inputs.get(key).cloned().ok_or(VarError::NotPresent)
            }
        },
        |get| ServerDatabaseConfig::from_lookup(16, 8, get),
    )
    .err()
    .ok_or("non-Unicode environment was accepted")?;
    assert!(matches!(&error, PostgresPoolConfigError::InvalidInput(key) if *key == invalid_key));
    assert_eq!(assert_redacted_chain(&error, &["private-", "�"]), 1);
    Ok(())
}

#[cfg(unix)]
#[rstest]
#[case("STAGE")]
#[case("POSTGRES_SSL_MODE")]
#[case("POSTGRES_SSL_ROOT_CERT")]
#[case("PGSSLCERT")]
#[case("PGSSLKEY")]
#[case("PGSSLROOTCERT")]
#[case("PGOPTIONS")]
fn should_reject_non_unicode_local_command_environment_before_bootstrap(
    #[case] invalid_key: &'static str,
) -> TestResult {
    let inputs = inputs("local");
    let error = parse_postgres_environment(
        |key| {
            if key == invalid_key {
                invalid_environment_value(b"/private-\xff-input")
            } else {
                inputs.get(key).cloned().ok_or(VarError::NotPresent)
            }
        },
        |get| LocalDevelopmentConfig::from_lookup("crawler-test", get),
    )
    .err()
    .ok_or("non-Unicode local command environment was accepted")?;
    assert!(
        matches!(&error, LocalDatabaseError::Config(PostgresPoolConfigError::InvalidInput(key)) if *key == invalid_key)
    );
    assert_eq!(assert_redacted_chain(&error, &["private-", "�"]), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_return_typed_non_unicode_error_even_if_parser_accepts_empty_marker() -> TestResult {
    let result = parse_postgres_environment(
        |_| invalid_environment_value(b"private-\xff-input"),
        |get| {
            assert_eq!(get("POSTGRES_SSL_ROOT_CERT"), Some(String::new()));
            Ok::<_, PostgresPoolConfigError>(())
        },
    );
    let error = result.err().ok_or("non-Unicode input was accepted")?;
    assert!(matches!(
        error,
        PostgresPoolConfigError::InvalidInput("POSTGRES_SSL_ROOT_CERT")
    ));
    assert_eq!(assert_redacted_chain(&error, &["private-", "�"]), 1);
    Ok(())
}

#[test]
fn should_use_strict_environment_adapter_at_every_postgres_entrypoint() {
    for source in [
        include_str!("../bin/server.rs"),
        include_str!("../bin/bootstrap-local.rs"),
        include_str!("../demo.rs"),
        include_str!("../spider/demo.rs"),
        include_str!("../scraper/demo.rs"),
    ] {
        assert!(source.contains("parse_postgres_environment("));
        assert!(!source.contains("var_os("));
        assert!(!source.contains("to_string_lossy("));
    }
}

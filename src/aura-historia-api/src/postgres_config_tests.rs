use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError as ConfigError};
use std::{collections::BTreeMap, error::Error};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn inputs() -> BTreeMap<&'static str, String> {
    [
        ("STAGE", "test"),
        ("POSTGRES_SSL_MODE", "disable"),
        ("POSTGRES_HOST", "postgres.example.test"),
        ("POSTGRES_DATABASE", "database_canary"),
        ("POSTGRES_USERNAME", "username_canary"),
        ("POSTGRES_PASSWORD", "password_canary"),
    ]
    .map(|(key, value)| (key, value.to_owned()))
    .into()
}

fn parse(values: &BTreeMap<&'static str, String>) -> TestResult<PostgresPoolConfig> {
    Ok(super::postgres_config(&mut |key| values.get(key).cloned())?)
}

fn assert_invalid(values: &BTreeMap<&'static str, String>, expected: ConfigError) -> TestResult {
    let error = parse(values)
        .err()
        .ok_or("expected PostgreSQL config rejection")?;
    let mut current: &(dyn Error + 'static) = error.as_ref();
    loop {
        let rendered = format!("{current} {current:?} {current:#?}");
        for canary in [
            "database_canary",
            "username_canary",
            "password_canary",
            "value_canary",
        ] {
            assert!(
                !rendered.contains(canary),
                "configuration error leaked an input"
            );
        }
        if let Some(config) = current.downcast_ref::<ConfigError>() {
            assert_eq!(config.to_string(), expected.to_string());
            return Ok(());
        }
        current = current
            .source()
            .ok_or("shared configuration cause missing")?;
    }
}

// Full startup/preflight ordering now uses the pure shared startup snapshot and
// real CLI subprocess tests in runtime_tests.rs / tests/cli.rs. PG policy stays here.

#[test]
fn should_use_explicit_local_tls_and_shared_pool_defaults() -> TestResult {
    for stage in ["local", "test", "ephemeral"] {
        let mut values = inputs();
        values.insert("STAGE", stage.into());
        let config = parse(&values)?;
        assert_eq!(config.host(), "postgres.example.test");
        assert_eq!(config.database(), "database_canary");
        assert_eq!(config.username(), "username_canary");
        assert_eq!(config.port(), 5432);
        assert_eq!(config.max_connections(), 2);
        assert_eq!(
            format!("{:?}", config.connect_options().get_ssl_mode()),
            "Disable"
        );
        assert_eq!(
            config.connect_options().get_application_name(),
            Some(env!("CARGO_PKG_NAME"))
        );
        assert_eq!(
            config.pool_options().get_acquire_timeout(),
            std::time::Duration::from_secs(5)
        );
        for canary in ["database_canary", "username_canary", "password_canary"] {
            assert!(!format!("{config:?}").contains(canary));
        }
    }
    Ok(())
}

#[test]
fn should_require_verify_full_with_a_ca_for_real_stages() -> TestResult {
    for stage in ["dev", "prod"] {
        let mut values = inputs();
        values.insert("STAGE", stage.into());
        assert_invalid(&values, ConfigError::VerifyFullRequired)?;
        values.insert("POSTGRES_SSL_MODE", "verify-full".into());
        assert_invalid(&values, ConfigError::RootCertificateRequired)?;
        values.insert(
            "POSTGRES_SSL_ROOT_CERT",
            concat!(env!("CARGO_MANIFEST_DIR"), "/src/postgres-test-ca.crt").into(),
        );
        values.insert("POSTGRES_PORT", "6543".into());
        values.insert("POSTGRES_MAX_CONNECTIONS", "7".into());
        let config = parse(&values)?;
        assert_eq!(
            format!("{:?}", config.connect_options().get_ssl_mode()),
            "VerifyFull"
        );
        assert_eq!(config.port(), 6543);
        assert_eq!(config.max_connections(), 7);
    }
    Ok(())
}

#[test]
fn should_reject_missing_unknown_and_malformed_postgres_inputs() -> TestResult {
    for (key, value, expected) in [
        ("STAGE", None, ConfigError::MissingInput("STAGE")),
        ("STAGE", Some("value_canary"), ConfigError::InvalidStage),
        ("STAGE", Some(""), ConfigError::InvalidStage),
        (
            "POSTGRES_SSL_MODE",
            None,
            ConfigError::MissingInput("POSTGRES_SSL_MODE"),
        ),
        (
            "POSTGRES_SSL_MODE",
            Some("prefer"),
            ConfigError::InvalidSslMode,
        ),
        (
            "POSTGRES_SSL_MODE",
            Some("require"),
            ConfigError::InvalidSslMode,
        ),
        (
            "POSTGRES_SSL_MODE",
            Some("verify-ca"),
            ConfigError::InvalidSslMode,
        ),
        ("POSTGRES_SSL_MODE", Some(""), ConfigError::InvalidSslMode),
        (
            "POSTGRES_SSL_ROOT_CERT",
            Some(""),
            ConfigError::InvalidInput("POSTGRES_SSL_ROOT_CERT"),
        ),
        (
            "POSTGRES_HOST",
            None,
            ConfigError::MissingInput("POSTGRES_HOST"),
        ),
        (
            "POSTGRES_HOST",
            Some(""),
            ConfigError::InvalidInput("POSTGRES_HOST"),
        ),
        (
            "POSTGRES_DATABASE",
            Some(""),
            ConfigError::InvalidInput("POSTGRES_DATABASE"),
        ),
        (
            "POSTGRES_USERNAME",
            Some(""),
            ConfigError::InvalidInput("POSTGRES_USERNAME"),
        ),
        (
            "POSTGRES_PASSWORD",
            Some(""),
            ConfigError::InvalidInput("POSTGRES_PASSWORD"),
        ),
        (
            "POSTGRES_PASSWORD",
            None,
            ConfigError::MissingInput("POSTGRES_PASSWORD or POSTGRES_PASSWORD_FILE"),
        ),
        (
            "POSTGRES_PASSWORD_FILE",
            Some("value_canary"),
            ConfigError::ConflictingPasswordInputs,
        ),
    ] {
        let mut values = inputs();
        match value {
            Some(value) => {
                values.insert(key, value.into());
            }
            None => {
                values.remove(key);
            }
        }
        assert_invalid(&values, expected)?;
    }
    Ok(())
}

#[test]
fn should_reject_invalid_pool_bounds_without_echoing_values() -> TestResult {
    for (key, values) in [
        (
            "POSTGRES_PORT",
            ["", "-1", "65536", " 5432 ", "value_canary"],
        ),
        (
            "POSTGRES_MAX_CONNECTIONS",
            ["", "-1", "4294967296", " 2 ", "value_canary"],
        ),
    ] {
        for value in values {
            let mut inputs = inputs();
            inputs.insert(key, value.into());
            assert_invalid(&inputs, ConfigError::InvalidInput(key))?;
        }
    }
    let mut values = inputs();
    values.insert("POSTGRES_MAX_CONNECTIONS", "0".into());
    assert_invalid(&values, ConfigError::ZeroMaxConnections)?;
    values.insert("POSTGRES_MAX_CONNECTIONS", "1".into());
    assert_eq!(parse(&values)?.max_connections(), 1);
    values.insert("POSTGRES_PORT", "0".into());
    assert_invalid(&values, ConfigError::InvalidInput("POSTGRES_PORT"))
}

#[test]
fn should_forward_unsupported_ambient_settings_to_shared_validation() -> TestResult {
    for key in ["PGSSLCERT", "PGSSLKEY", "PGSSLROOTCERT", "PGOPTIONS"] {
        for value in ["value_canary", ""] {
            let mut values = inputs();
            values.insert(key, value.into());
            assert_invalid(&values, ConfigError::UnsupportedAmbientSetting(key))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_read_only_a_protected_password_file_when_selected() -> TestResult {
    use std::{
        fs,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    struct PasswordFile(std::path::PathBuf);
    impl Drop for PasswordFile {
        fn drop(&mut self) {
            if fs::remove_file(&self.0).is_err() {
                eprintln!("password fixture cleanup failed (path suppressed)");
            }
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        ".postgres-password-test-{}-{stamp}",
        std::process::id()
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    let fixture = PasswordFile(path);
    file.write_all(b"password_canary\n")?;
    drop(file);
    let mut values = inputs();
    values.remove("POSTGRES_PASSWORD");
    values.insert(
        "POSTGRES_PASSWORD_FILE",
        fixture.0.to_str().ok_or("invalid fixture path")?.into(),
    );
    for mode in [0o600, 0o400] {
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(mode))?;
        assert!(parse(&values).is_ok());
    }
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o644))?;
    assert_invalid(&values, ConfigError::SecretFilePermissions)?;
    values.insert("POSTGRES_PASSWORD_FILE", "".into());
    assert_invalid(&values, ConfigError::InvalidInput("POSTGRES_PASSWORD_FILE"))
}

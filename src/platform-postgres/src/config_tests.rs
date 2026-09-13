use super::*;
use crate::test_support::{TestDirectory, TestResult, root_pem};
use rstest::rstest;
use std::{collections::HashMap, error::Error, process::Command};

fn inputs() -> HashMap<&'static str, String> {
    [
        ("STAGE", "test"),
        ("POSTGRES_SSL_MODE", "disable"),
        ("POSTGRES_HOST", "localhost"),
        ("POSTGRES_DATABASE", "policy_db"),
        ("POSTGRES_USERNAME", "private-user"),
        ("POSTGRES_PASSWORD", "private-password"),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.to_owned()))
    .collect()
}

fn local_tls() -> Result<PostgresTlsConfig, PostgresPoolConfigError> {
    PostgresTlsConfig::new("test", "disable", None, "policy-tests")
}

#[rstest]
#[case("dev")]
#[case("prod")]
fn should_require_authenticated_tls_for_real_stage(#[case] stage: &str) -> TestResult {
    assert!(matches!(
        PostgresTlsConfig::new(stage, "disable", None, "policy-tests"),
        Err(PostgresPoolConfigError::VerifyFullRequired)
    ));
    assert!(matches!(
        PostgresTlsConfig::new(stage, "verify-full", None, "policy-tests"),
        Err(PostgresPoolConfigError::RootCertificateRequired)
    ));
    let tls = PostgresTlsConfig::new(stage, "verify-full", Some(root_pem()?), "policy-tests")?;
    let config = PostgresPoolConfig::new(
        "localhost".into(),
        5432,
        "policy_db".into(),
        "private-user".into(),
        "private-password".into(),
        2,
        tls,
    )?;
    assert!(matches!(
        config.connect_options().get_ssl_mode(),
        PgSslMode::VerifyFull
    ));
    assert_eq!(
        config.connect_options().get_application_name(),
        Some("policy-tests")
    );
    assert!(config.connect_options().get_socket().is_none());
    Ok(())
}

#[rstest]
#[case("local")]
#[case("ephemeral")]
#[case("test")]
fn should_allow_plaintext_only_for_explicit_non_real_stage(#[case] stage: &str) -> TestResult {
    assert!(matches!(
        PostgresTlsConfig::new(stage, "disable", None, "policy-tests")?.mode,
        PgSslMode::Disable
    ));
    assert!(matches!(
        PostgresTlsConfig::new(stage, "verify-full", None, "policy-tests"),
        Err(PostgresPoolConfigError::RootCertificateRequired)
    ));
    assert!(matches!(
        PostgresTlsConfig::new(stage, "disable", Some(vec![]), "policy-tests"),
        Err(PostgresPoolConfigError::UnexpectedRootCertificate)
    ));
    Ok(())
}

#[rstest]
#[case("")]
#[case(" ")]
#[case("development")]
#[case("production")]
#[case("DEV")]
#[case("Prod")]
#[case("tests")]
#[case("unknown")]
fn should_reject_unknown_stage(#[case] stage: &str) {
    assert!(matches!(
        PostgresTlsConfig::new(stage, "disable", None, "policy-tests"),
        Err(PostgresPoolConfigError::InvalidStage)
    ));
}

#[rstest]
#[case("")]
#[case("prefer")]
#[case("allow")]
#[case("require")]
#[case("verify-ca")]
#[case("VERIFY_FULL")]
#[case("VerifyFull")]
#[case("verify-full ")]
#[case("off")]
fn should_reject_unsupported_mode(#[case] mode: &str) {
    assert!(matches!(
        PostgresTlsConfig::new("test", mode, None, "policy-tests"),
        Err(PostgresPoolConfigError::InvalidSslMode)
    ));
}

#[rstest]
#[case("")]
#[case("a b")]
#[case("secret@host")]
#[case("app\n")]
#[case("ä")]
#[case("-app")]
#[case("a:b")]
fn should_reject_unsafe_application_name(#[case] app: &str) {
    assert!(matches!(
        PostgresTlsConfig::new("test", "disable", None, app),
        Err(PostgresPoolConfigError::InvalidApplicationName)
    ));
}

#[test]
fn should_enforce_application_name_byte_limit() {
    assert!(PostgresTlsConfig::new("test", "disable", None, &"a".repeat(63)).is_ok());
    assert!(PostgresTlsConfig::new("test", "disable", None, &"a".repeat(64)).is_err());
    assert!(PostgresTlsConfig::new("test", "disable", None, "Aura.api_worker-01").is_ok());
}

#[test]
fn should_parse_structured_real_minimum_and_public_ca_file() -> TestResult {
    let directory = TestDirectory::new()?;
    let ca = directory.file("ca.crt", &root_pem()?, 0o644)?;
    let password = directory.file("password", b" private-password \r\n", 0o400)?;
    let mut values = inputs();
    values.insert("STAGE", "prod".into());
    values.insert("POSTGRES_SSL_MODE", "verify-full".into());
    values.insert("POSTGRES_SSL_ROOT_CERT", ca.to_string_lossy().into_owned());
    values.remove("POSTGRES_PASSWORD");
    values.insert(
        "POSTGRES_PASSWORD_FILE",
        password.to_string_lossy().into_owned(),
    );
    let config = PostgresPoolConfig::from_lookup("policy-tests", |key| values.remove(key))?;
    assert_eq!(config.port(), 5432);
    assert_eq!(config.max_connections(), 2);
    assert_eq!(config.pool_options().get_max_connections(), 2);
    assert!(matches!(
        config.connect_options().get_ssl_mode(),
        PgSslMode::VerifyFull
    ));
    let options = config.connect_options().to_url_lossy();
    let preserved = options.password().map(decode).transpose()?;
    assert!(preserved.as_deref() == Some(" private-password "));
    assert!(values.is_empty());
    Ok(())
}

#[rstest]
#[case("STAGE")]
#[case("POSTGRES_SSL_MODE")]
#[case("POSTGRES_HOST")]
#[case("POSTGRES_DATABASE")]
#[case("POSTGRES_USERNAME")]
#[case("POSTGRES_PASSWORD")]
fn should_reject_missing_or_empty_required_input(#[case] key: &'static str) {
    let mut values = inputs();
    values.remove(key);
    assert!(
        PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned()).is_err()
    );
    values.insert(key, String::new());
    assert!(
        PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned()).is_err()
    );
}

#[rstest]
#[case("POSTGRES_PORT", "0")]
#[case("POSTGRES_PORT", "65536")]
#[case("POSTGRES_PORT", "")]
#[case("POSTGRES_PORT", "+5432")]
#[case("POSTGRES_MAX_CONNECTIONS", "0")]
#[case("POSTGRES_MAX_CONNECTIONS", "4294967296")]
#[case("POSTGRES_MAX_CONNECTIONS", "-1")]
#[case("POSTGRES_MAX_CONNECTIONS", "two")]
fn should_reject_invalid_pool_number(#[case] key: &'static str, #[case] value: &str) {
    let mut values = inputs();
    values.insert(key, value.to_owned());
    assert!(
        PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned()).is_err()
    );
}

#[test]
fn should_accept_explicit_pool_numbers_and_reject_zero_direct_cap() -> TestResult {
    let mut values = inputs();
    values.insert("POSTGRES_PORT", "6543".into());
    values.insert("POSTGRES_MAX_CONNECTIONS", "4".into());
    let config = PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned())?;
    assert_eq!(config.port(), 6543);
    assert_eq!(config.max_connections(), 4);
    assert!(matches!(
        PostgresPoolConfig::from_url("postgres://u:p@localhost/db", 0, local_tls()?),
        Err(PostgresPoolConfigError::ZeroMaxConnections)
    ));
    Ok(())
}

#[rstest]
#[case("")]
#[case("/var/run/postgresql")]
#[case("%2Ftmp")]
#[case("host:5432")]
#[case("host/path")]
#[case("host user")]
#[case("-host")]
#[case("host..name")]
fn should_reject_non_tcp_or_invalid_structured_host(#[case] host: &str) -> TestResult {
    assert!(
        PostgresPoolConfig::new(
            host.into(),
            5432,
            "db".into(),
            "u".into(),
            "p".into(),
            2,
            local_tls()?
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn should_reject_malformed_ca_and_accept_certificate_bundles() -> TestResult {
    for pem in [
        vec![],
        b"garbage".to_vec(),
        b"-----BEGIN CERTIFICATE-----\nYWJj\n-----END CERTIFICATE-----".to_vec(),
        b"-----BEGIN CERTIFICATE-----\n???\n-----END CERTIFICATE-----".to_vec(),
        b"-----BEGIN CERTIFICATE-----\nYWJj".to_vec(),
        vec![255],
    ] {
        assert!(PostgresTlsConfig::new("prod", "verify-full", Some(pem), "policy-tests").is_err());
    }
    let root = root_pem()?;
    let mut bundle = root.clone();
    bundle.extend_from_slice(&root);
    assert!(PostgresTlsConfig::new("prod", "verify-full", Some(bundle), "policy-tests").is_ok());
    for extra in [
        b"\n-----BEGIN PRIVATE KEY-----\nYWJj\n-----END PRIVATE KEY-----".as_slice(),
        b"trailing garbage",
    ] {
        let mut mixed = root.clone();
        mixed.extend_from_slice(extra);
        assert!(
            PostgresTlsConfig::new("prod", "verify-full", Some(mixed), "policy-tests").is_err()
        );
    }
    assert!(matches!(
        PostgresTlsConfig::new(
            "prod",
            "verify-full",
            Some(vec![b' '; MAX_ROOT_CERT_BYTES + 1]),
            "policy-tests"
        ),
        Err(PostgresPoolConfigError::RootCertificateTooLarge)
    ));
    Ok(())
}

#[test]
fn should_reject_conflicting_password_inputs_even_when_empty() {
    let mut values = inputs();
    values.insert("POSTGRES_PASSWORD_FILE", String::new());
    assert!(matches!(
        PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned()),
        Err(PostgresPoolConfigError::ConflictingPasswordInputs)
    ));
}

#[test]
fn should_bound_files_and_reject_missing_empty_nonregular_or_unreadable_inputs() -> TestResult {
    let directory = TestDirectory::new()?;
    let missing = directory.0.join("sensitive-path-never-log");
    for (key, secret, max) in [
        ("POSTGRES_PASSWORD_FILE", true, MAX_PASSWORD_BYTES),
        ("POSTGRES_SSL_ROOT_CERT", false, MAX_ROOT_CERT_BYTES),
    ] {
        assert!(read_file("", key, max, secret).is_err());
        let error = read_file(&missing.to_string_lossy(), key, max, secret)
            .err()
            .ok_or("missing file accepted")?;
        assert!(!format!("{error:?} {error}").contains("sensitive-path-never-log"));
        assert!(error.source().is_some());
        assert!(read_file(&directory.0.to_string_lossy(), key, max, secret).is_err());
        let empty = directory.file("empty", b"", 0o600)?;
        assert!(read_file(&empty.to_string_lossy(), key, max, secret).is_err());
        let large = directory.file("large", &vec![b'x'; max + 1], 0o600)?;
        assert!(matches!(
            read_file(&large.to_string_lossy(), key, max, secret),
            Err(PostgresPoolConfigError::FileTooLarge(_))
        ));
        let unreadable = directory.file(&format!("unreadable-{key}"), b"x", 0o000)?;
        assert!(read_file(&unreadable.to_string_lossy(), key, max, secret).is_err());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn should_reject_public_password_symlinks_and_fifos_without_blocking() -> TestResult {
    use std::os::unix::fs::symlink;
    let directory = TestDirectory::new()?;
    let file = directory.file("password", b"p", 0o644)?;
    assert!(matches!(
        read_file(
            &file.to_string_lossy(),
            "POSTGRES_PASSWORD_FILE",
            MAX_PASSWORD_BYTES,
            true
        ),
        Err(PostgresPoolConfigError::SecretFilePermissions)
    ));
    let link = directory.0.join("link");
    symlink(&file, &link)?;
    for secret in [true, false] {
        assert!(
            read_file(
                &link.to_string_lossy(),
                "POSTGRES_PASSWORD_FILE",
                MAX_PASSWORD_BYTES,
                secret
            )
            .is_err()
        );
    }
    let fifo = directory.0.join("fifo");
    crate::test_support::run(Command::new("timeout").args(["5s", "mkfifo"]).arg(&fifo))?;
    assert!(matches!(
        read_file(
            &fifo.to_string_lossy(),
            "POSTGRES_PASSWORD_FILE",
            MAX_PASSWORD_BYTES,
            true
        ),
        Err(PostgresPoolConfigError::NotRegularFile(_))
    ));
    Ok(())
}

#[rstest]
#[case(b"\n".as_slice())]
#[case(b"\r\n".as_slice())]
#[case(b"p\nsecond".as_slice())]
#[case(b"p\0".as_slice())]
#[case(b"\xff".as_slice())]
fn should_reject_invalid_password_file_content(#[case] content: &[u8]) -> TestResult {
    let directory = TestDirectory::new()?;
    let path = directory.file("password", content, 0o600)?;
    let mut values = inputs();
    values.remove("POSTGRES_PASSWORD");
    values.insert(
        "POSTGRES_PASSWORD_FILE",
        path.to_string_lossy().into_owned(),
    );
    assert!(
        PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned()).is_err()
    );
    Ok(())
}

#[test]
fn should_parse_url_real_minimum_and_preserve_protected_policy() -> TestResult {
    let tls = PostgresTlsConfig::new("prod", "verify-full", Some(root_pem()?), "policy-tests")?;
    let config = PostgresPoolConfig::from_url(
        "postgresql://private-user:p%40ss%3Aword@localhost/policy_db?sslmode=verify-full&application_name=policy-tests",
        3,
        tls,
    )?;
    let options = config.connect_options();
    assert!(matches!(options.get_ssl_mode(), PgSslMode::VerifyFull));
    assert_eq!(options.get_application_name(), Some("policy-tests"));
    assert_eq!(config.database(), "policy_db");
    assert_eq!(config.username(), "private-user");
    assert_eq!(config.max_connections(), 3);
    let password = options.to_url_lossy().password().map(decode).transpose()?;
    assert!(password.as_deref() == Some("p@ss:word"));
    let ipv6 = PostgresPoolConfig::from_url("postgres://u:p@[::1]:6543/db", 2, local_tls()?)?;
    assert_eq!(ipv6.host(), "::1");
    assert_eq!(ipv6.port(), 6543);
    Ok(())
}

#[rstest]
#[case("postgres://u:p@localhost/db?sslmode=disable")]
#[case("postgres://u:p@localhost/db?sslmode=require")]
#[case("postgres://u:p@localhost/db?sslmode=verify-full&sslmode=verify-full")]
#[case("postgres://u:p@localhost/db?sslmode=verify-full&%73slmode=verify-full")]
#[case("postgres://u:p@localhost/db?application_name=other")]
#[case("postgres://u:p@localhost/db?application_name=policy-tests&application_name=policy-tests")]
#[case("postgres://u:p@localhost/db?sslrootcert=secret-path")]
#[case("postgres://u:p@localhost/db?ssl-ca=secret-path")]
#[case("postgres://u:p@localhost/db?sslcert=secret-path")]
#[case("postgres://u:p@localhost/db?sslkey=secret-path")]
#[case("postgres://u:p@localhost/db?options=-c%20search_path%3Devil")]
#[case("postgres://u:p@localhost/db?host=other")]
#[case("postgres://u:p@localhost/db?password=other")]
#[case("postgres://u:p@localhost/db?unknown=secret")]
#[case("postgres://u:p@localhost/db?ssl-mode=verify-full")]
fn should_reject_url_policy_bypass(#[case] url: &str) -> TestResult {
    let tls = PostgresTlsConfig::new("prod", "verify-full", Some(root_pem()?), "policy-tests")?;
    assert!(PostgresPoolConfig::from_url(url, 2, tls).is_err());
    Ok(())
}

#[rstest]
#[case("localhost/db")]
#[case("mysql://u:p@localhost/db")]
#[case("postgres:///db")]
#[case("postgres://u:p@/db")]
#[case("postgres://localhost/db")]
#[case("postgres://u@localhost/db")]
#[case("postgres://u:@localhost/db")]
#[case("postgres://:p@localhost/db")]
#[case("postgres://u:p@localhost")]
#[case("postgres://u:p@localhost/")]
#[case("postgres://u:p@localhost:/db")]
#[case("postgres://u:p@localhost:0/db")]
#[case("postgres://u:p@localhost:65536/db")]
#[case("postgres://u:p@localhost/db/extra")]
#[case("postgres://u:p@localhost/a/../db")]
#[case("postgres://u:p@localhost/%2E%2E")]
#[case("postgres://u:p@localhost/db#fragment")]
#[case("postgres://u:p@localhost/db?")]
#[case("postgres://u:p@localhost/db?sslmode")]
#[case("postgres://u:%zz@localhost/db")]
#[case("postgres://u:%ff@localhost/db")]
#[case("postgres://u:p@localhost/%00")]
#[case(" postgres://u:p@localhost/db")]
#[case("postgres://u:p@local\nhost/db")]
#[case("postgres://u:p@%2Ftmp/db")]
fn should_reject_implicit_or_malformed_url(#[case] url: &str) -> TestResult {
    let config = PostgresPoolConfig::from_url(url, 2, local_tls()?);
    assert!(config.is_err());
    Ok(())
}

#[test]
fn should_redact_config_and_connection_errors_but_retain_source() -> TestResult {
    let tls = PostgresTlsConfig::new("prod", "verify-full", Some(root_pem()?), "private-app")?;
    assert!(!format!("{tls:?}").contains("private-app"));
    let config = PostgresPoolConfig::from_url(
        "postgres://private-user:private-password@localhost/private-db",
        2,
        tls,
    )?;
    let output = format!("{config:?}");
    for private in [
        "private-user",
        "private-password",
        "private-db",
        "private-app",
        "BEGIN CERTIFICATE",
        "postgres://",
    ] {
        assert!(!output.contains(private));
    }
    let error = PostgresConnectError::from(sqlx::Error::Protocol(
        "private-user private-password private-provider-body".into(),
    ));
    crate::test_support::assert_redacted_chain(
        &error,
        &["private-user", "private-password", "private-provider-body"],
    );
    assert!(
        matches!(&error.source.original, sqlx::Error::Protocol(message) if message.contains("private-provider-body"))
    );
    let invalid = PostgresPoolConfig::from_url(
        "postgres://private-user:private-password@localhost/db?private-key=private-value",
        2,
        local_tls()?,
    );
    assert!(!format!("{invalid:?}").contains("private-"));
    Ok(())
}

#[test]
fn should_redact_all_source_variants_without_discarding_original_causes() {
    const CANARY: &str = "private-provider-username-password-canary";
    let originals = [
        sqlx::Error::Configuration(CANARY.into()),
        sqlx::Error::Tls(CANARY.into()),
        sqlx::Error::Io(io::Error::other(CANARY)),
        sqlx::Error::Io(io::Error::new(io::ErrorKind::TimedOut, CANARY)),
        sqlx::Error::Protocol(CANARY.into()),
        sqlx::Error::PoolTimedOut,
        sqlx::Error::PoolClosed,
    ];
    for original in originals {
        let expected = std::mem::discriminant(&original);
        let error = PostgresConnectError::from(original);
        assert_eq!(std::mem::discriminant(&error.source.original), expected);
        crate::test_support::assert_redacted_chain(&error, &[CANARY]);
        assert!(
            error
                .source()
                .is_some_and(|source| source.source().is_none())
        );
    }
}

#[tokio::test]
async fn should_time_out_connect_session_after_five_seconds_when_peer_stalls() -> TestResult {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let tls = PostgresTlsConfig::new("prod", "verify-full", Some(root_pem()?), "policy-tests")?;
    let config = PostgresPoolConfig::new(
        address.ip().to_string(),
        address.port(),
        "db".into(),
        "stalled-user-canary".into(),
        "stalled-password-canary".into(),
        1,
        tls,
    )?;
    assert_eq!(
        config.pool_options().get_acquire_timeout(),
        Duration::from_secs(5)
    );
    let started = tokio::time::Instant::now();
    // join! retains the accepted socket without replying while the TLS client waits.
    let (result, accepted) = tokio::time::timeout(Duration::from_secs(7), async {
        tokio::join!(config.connect_session(), listener.accept())
    })
    .await
    .map_err(|_| "connect_session exceeded its five-second deadline")?;
    let (_socket, _) = accepted?;
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(7));
    let error = result
        .err()
        .ok_or("stalled session unexpectedly connected")?;
    crate::test_support::assert_redacted_chain(
        &error,
        &["stalled-user-canary", "stalled-password-canary"],
    );
    assert_eq!(
        error
            .source()
            .ok_or("missing classified timeout")?
            .to_string(),
        "PostgreSQL connection timed out"
    );
    assert!(
        matches!(&error.source.original, sqlx::Error::Io(cause) if cause.kind() == io::ErrorKind::TimedOut)
    );
    Ok(())
}

#[test]
fn should_keep_cached_options_when_inputs_files_and_returned_clones_change() -> TestResult {
    let directory = TestDirectory::new()?;
    let ca = directory.file("ca.crt", &root_pem()?, 0o644)?;
    let password = directory.file("password", b"original-password", 0o600)?;
    let mut values = inputs();
    values.insert("STAGE", "prod".into());
    values.insert("POSTGRES_SSL_MODE", "verify-full".into());
    values.insert("POSTGRES_SSL_ROOT_CERT", ca.to_string_lossy().into_owned());
    values.remove("POSTGRES_PASSWORD");
    values.insert(
        "POSTGRES_PASSWORD_FILE",
        password.to_string_lossy().into_owned(),
    );
    let config = PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned())?;
    let before = config.connect_options().to_url_lossy();
    let clone = config.clone();
    values.insert("PGOPTIONS", "private-later-options".into());
    values.insert("POSTGRES_HOST", "changed.invalid".into());
    directory.file("ca.crt", b"invalid replacement PEM", 0o644)?;
    directory.file("password", b"replacement-password", 0o600)?;
    let changed = config
        .connect_options()
        .host("changed.invalid")
        .username("changed")
        .password("replacement-password")
        .ssl_root_cert_from_pem(vec![])
        .ssl_mode(PgSslMode::Disable)
        .application_name("changed");
    assert!(changed.to_url_lossy() != before);
    assert!(config.connect_options().to_url_lossy() == before);
    assert!(clone.connect_options().to_url_lossy() == before);
    assert!(matches!(
        config.connect_options().get_ssl_mode(),
        PgSslMode::VerifyFull
    ));
    assert_eq!(
        config.connect_options().get_application_name(),
        Some("policy-tests")
    );
    assert!(
        PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned()).is_err()
    );
    Ok(())
}

#[test]
fn should_reject_unsupported_ambient_lookup_even_when_empty() {
    for key in UNSUPPORTED_AMBIENT {
        for value in ["", "private-input"] {
            let mut values = inputs();
            values.insert(key, value.into());
            assert!(matches!(
                PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned()),
                Err(PostgresPoolConfigError::UnsupportedAmbientSetting(_))
            ));
        }
    }
}

#[test]
fn should_isolate_actual_sqlx_environment_checks_in_child_processes() -> TestResult {
    for (key, value) in UNSUPPORTED_AMBIENT
        .into_iter()
        .flat_map(|key| [(key, ""), (key, "private-ambient-value")])
        .chain([("PGDEFAULTS", "")])
    {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                "--exact",
                "config::tests::should_check_actual_ambient_in_child",
                "--ignored",
            ])
            .env_clear()
            .env("POLICY_CHILD_KEY", key);
        if key == "PGDEFAULTS" {
            command
                .env("PGHOSTADDR", "host with invalid authority/")
                .env("PGHOST", "another invalid authority/")
                .env("PGPORT", "0")
                .env("PGUSER", "@[]/")
                .env("PGDATABASE", "private-ambient-db")
                .env("PGPASSWORD", "private-ambient-password")
                .env("PGSSLMODE", "require")
                .env("PGAPPNAME", "private-ambient-app")
                .env("PGPASSFILE", "/missing/private-pgpass");
        } else {
            command.env(key, value);
        }
        let output = crate::test_support::run(&mut command)?;
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
    Ok(())
}

#[test]
#[ignore = "subprocess helper; run by should_isolate_actual_sqlx_environment_checks_in_child_processes"]
fn should_check_actual_ambient_in_child() -> TestResult {
    let key = std::env::var("POLICY_CHILD_KEY")?;
    let mut values = inputs();
    values.extend(std::env::vars().filter_map(|(key, value)| {
        UNSUPPORTED_AMBIENT
            .into_iter()
            .find(|known| *known == key)
            .map(|key| (key, value))
    }));
    let structured =
        PostgresPoolConfig::from_lookup("policy-tests", |key| values.get(key).cloned());
    let typed = PostgresPoolConfig::new(
        "localhost".into(),
        5432,
        "db".into(),
        "u".into(),
        "p".into(),
        2,
        local_tls()?,
    );
    let config = PostgresPoolConfig::from_url("postgres://u:p@localhost/db", 2, local_tls()?);
    if key == "PGDEFAULTS" {
        assert!(typed.is_ok());
        assert!(structured.is_ok());
        let config = config?;
        assert_eq!(config.host(), "localhost");
        assert_eq!(config.port(), 5432);
        assert_eq!(config.username(), "u");
        assert_eq!(config.database(), "db");
        assert!(matches!(
            config.connect_options().get_ssl_mode(),
            PgSslMode::Disable
        ));
        assert_eq!(
            config.connect_options().get_application_name(),
            Some("policy-tests")
        );
    } else {
        for result in [config, typed, structured] {
            assert!(
                matches!(result, Err(PostgresPoolConfigError::UnsupportedAmbientSetting(input)) if input == key)
            );
        }
        let environment: HashMap<String, String> = std::env::vars().collect();
        assert!(matches!(
            PostgresTlsConfig::from_lookup("policy-tests", |key| environment.get(key).cloned()),
            Err(PostgresPoolConfigError::UnsupportedAmbientSetting(_))
        ));
    }
    Ok(())
}

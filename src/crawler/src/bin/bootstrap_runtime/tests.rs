use super::*;
use std::{
    collections::BTreeMap,
    env::VarError,
    error::Error,
    io::{Read, Write},
    net::TcpListener,
};

type TestResult = Result<(), Box<dyn Error>>;
const SECRET: &str = "sentinel_password_provider_body_private_path";

fn environment(url: &str) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("STAGE", "test".into()),
        ("POSTGRES_SSL_MODE", "disable".into()),
        ("LOCAL_DB_URL", url.into()),
        ("BUSINESS_DATABASE_URL", url.into()),
    ])
}

fn load(
    target: Target,
    initialize: bool,
    values: &BTreeMap<&'static str, String>,
) -> Result<PostgresPoolConfig, Failure> {
    config::load(target, initialize, |key| {
        values.get(key).cloned().ok_or(VarError::NotPresent)
    })
}

fn safe_chain(error: &(dyn Error + 'static)) {
    let mut next = Some(error);
    let mut count = 0;
    while let Some(error) = next {
        let rendered = format!("{error} {error:?} {error:#?}");
        for forbidden in [
            SECRET,
            "postgres://",
            "private_user",
            "private_db",
            "/private/",
        ] {
            assert!(!rendered.contains(forbidden));
        }
        count += 1;
        assert!(count <= 3);
        next = error.source();
    }
}

#[test]
fn should_parse_only_frozen_cli() -> TestResult {
    for (args, expected) in [
        (vec![], Command::Legacy),
        (vec!["--help"], Command::Help),
        (
            vec!["--initialize-fresh", "business"],
            Command::Initialize(Target::Business),
        ),
        (
            vec!["--initialize-fresh", "crawler"],
            Command::Initialize(Target::Crawler),
        ),
        (
            vec!["--verify", "business"],
            Command::Verify(Target::Business),
        ),
        (
            vec!["--verify", "crawler"],
            Command::Verify(Target::Crawler),
        ),
    ] {
        assert_eq!(parse(args.into_iter().map(OsString::from))?, expected);
    }
    Ok(())
}

#[test]
fn should_reject_invalid_cli_before_environment_or_runtime() {
    for args in [
        vec![SECRET],
        vec!["--help", SECRET],
        vec!["--verify"],
        vec!["--initialize-fresh"],
        vec!["--verify", "both"],
        vec!["--initialize-fresh", "crawler", SECRET],
        vec!["--verify", "business", "--help"],
        vec!["--"],
        vec!["--verify=business"],
        vec!["--initialize-fresh", "BUSINESS"],
    ] {
        let mut writes = false;
        let result = dispatch(args.into_iter().map(OsString::from), &mut writes);
        assert!(matches!(
            result,
            Err(Failure {
                code: Code::Usage,
                ..
            })
        ));
        assert!(!writes);
    }
}

#[cfg(unix)]
#[test]
fn should_reject_nonunicode_cli_and_selected_env_without_lossy_conversion() -> TestResult {
    use std::os::unix::ffi::OsStringExt;
    let invalid = OsString::from_vec(vec![0xff]);
    for args in [
        vec![invalid.clone()],
        vec!["--verify".into(), invalid.clone()],
        vec!["--help".into(), invalid.clone()],
    ] {
        assert!(matches!(
            parse(args),
            Err(Failure {
                code: Code::Usage,
                ..
            })
        ));
    }
    for key in ["STAGE", "POSTGRES_SSL_MODE", "LOCAL_DB_URL", "PGOPTIONS"] {
        let values = environment("postgres://private_user:password@127.0.0.1/private_db");
        let result = config::load(Target::Crawler, true, |name| {
            if name == key {
                Err(VarError::NotUnicode(invalid.clone()))
            } else {
                values.get(name).cloned().ok_or(VarError::NotPresent)
            }
        });
        match result {
            Err(error) => safe_chain(&error),
            Ok(_) => return Err("accepted nonunicode input".into()),
        }
    }
    Ok(())
}

#[test]
fn should_load_only_selected_url_and_shared_config() -> TestResult {
    for target in [Target::Business, Target::Crawler] {
        let selected = match target {
            Target::Business => "BUSINESS_DATABASE_URL",
            Target::Crawler => "LOCAL_DB_URL",
        };
        let other = match target {
            Target::Business => "LOCAL_DB_URL",
            Target::Crawler => "BUSINESS_DATABASE_URL",
        };
        let mut values =
            environment("postgres://private_user:explicit_password@127.0.0.1/private_db");
        values.remove(other);
        let mut read = Vec::new();
        let config = config::load(target, true, |key| {
            read.push(key);
            values.get(key).cloned().ok_or(VarError::NotPresent)
        })?;
        assert!(read.contains(&selected));
        assert!(!read.contains(&other));
        assert_eq!(config.max_connections(), 1);
        assert!(!format!("{config:#?}").contains("private_user"));
        assert!(matches!(
            config.connect_options().get_ssl_mode(),
            sqlx::postgres::PgSslMode::Disable
        ));
        assert_eq!(
            config.connect_options().get_application_name(),
            Some("crawler-bootstrap-local")
        );
    }
    Ok(())
}

#[test]
fn should_require_explicit_stage_tls_credentials_database_and_local_initialization_host()
-> TestResult {
    let url = "postgres://private_user:explicit_password@127.0.0.1/private_db";
    for target in [Target::Business, Target::Crawler] {
        for initialize in [false, true] {
            for key in [
                "STAGE",
                "POSTGRES_SSL_MODE",
                match target {
                    Target::Business => "BUSINESS_DATABASE_URL",
                    Target::Crawler => "LOCAL_DB_URL",
                },
            ] {
                let mut values = environment(url);
                values.remove(key);
                assert!(load(target, initialize, &values).is_err());
            }
            for stage in ["", "dev", "prod", "unknown", "LOCAL", " test"] {
                let mut values = environment(url);
                values.insert("STAGE", stage.into());
                assert!(matches!(
                    load(target, initialize, &values),
                    Err(Failure {
                        code: Code::UnsupportedStage,
                        ..
                    })
                ));
            }
        }
    }
    for stage in ["local", "ephemeral", "test"] {
        for host in ["localhost", "127.0.0.1", "127.9.8.7", "[::1]"] {
            let mut values = environment(&format!(
                "postgres://private_user:password@{host}/private_db"
            ));
            values.insert("STAGE", stage.into());
            load(Target::Crawler, true, &values)?;
        }
    }
    for host in [
        "localhost.",
        "LOCALHOST",
        "example.invalid",
        "10.0.0.1",
        "0.0.0.0",
        "[::]",
        "[::ffff:127.0.0.1]",
    ] {
        let values = environment(&format!(
            "postgres://private_user:password@{host}/private_db"
        ));
        assert!(load(Target::Crawler, true, &values).is_err());
    }
    // Verification is read-only and may use an explicit remote URL in a non-real stage.
    load(
        Target::Crawler,
        false,
        &environment("postgres://private_user:password@example.invalid/private_db"),
    )?;
    for bad in [
        "postgres://127.0.0.1/private_db",
        "postgres://private_user@127.0.0.1/private_db",
        "postgres://private_user:@127.0.0.1/private_db",
        "postgres://:password@127.0.0.1/private_db",
        "postgres://private_user:password@127.0.0.1/",
        "postgres://private_user:password@127.0.0.1/private_db?host=remote",
        "postgres://private_user:password@127.0.0.1/private_db?sslmode=require",
        "postgres://private_user:password@127.0.0.1/private_db?application_name=other",
    ] {
        assert!(load(Target::Crawler, true, &environment(bad)).is_err());
    }
    Ok(())
}

#[test]
fn should_refuse_ambient_tls_and_redact_config_files_and_values() -> TestResult {
    for (key, value) in [
        ("PGOPTIONS", ""),
        ("PGSSLCERT", SECRET),
        ("PGSSLKEY", SECRET),
        ("PGSSLROOTCERT", SECRET),
        ("POSTGRES_SSL_MODE", SECRET),
        ("LOCAL_DB_URL", SECRET),
        (
            "POSTGRES_SSL_ROOT_CERT",
            "/private/sentinel_password_provider_body_private_path",
        ),
    ] {
        let mut values = environment("postgres://private_user:password@127.0.0.1/private_db");
        values.insert(key, value.into());
        match load(Target::Crawler, true, &values) {
            Err(error) => safe_chain(&error),
            Ok(_) => return Err("accepted unsupported config".into()),
        }
    }
    Ok(())
}

#[test]
fn should_require_and_retain_shared_verify_full_ca_policy() -> TestResult {
    let mut values =
        environment("postgres://private_user:password@localhost/private_db?sslmode=verify-full");
    values.insert("POSTGRES_SSL_MODE", "verify-full".into());
    assert!(load(Target::Crawler, true, &values).is_err());
    values.insert(
        "POSTGRES_SSL_ROOT_CERT",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/crawler-postgres-ca.pem"
        )
        .into(),
    );
    let config = load(Target::Crawler, true, &values)?;
    assert!(matches!(
        config.connect_options().get_ssl_mode(),
        sqlx::postgres::PgSslMode::VerifyFull
    ));
    values.insert(
        "LOCAL_DB_URL",
        "postgres://private_user:password@localhost/private_db?sslmode=disable".into(),
    );
    assert!(load(Target::Crawler, true, &values).is_err());
    Ok(())
}

#[test]
fn should_hide_every_raw_cause_and_classify_all_postwrite_failures_unknown() {
    for code in [
        Code::Usage,
        Code::Config,
        Code::UnsupportedStage,
        Code::UnsupportedPlatform,
        Code::NonLocalEndpoint,
        Code::NotFresh,
        Code::Prerequisite,
        Code::UnsupportedSource,
        Code::Dependency,
        Code::Verification,
        Code::Deadline,
        Code::Cleanup,
        Code::UnknownOutcome,
        Code::Runtime,
        Code::Output,
        Code::Legacy,
    ] {
        for possible in [false, true] {
            let error = Failure::caused(code, sqlx::Error::Io(std::io::Error::other(SECRET)))
                .after_writes(possible);
            safe_chain(&error);
            assert_eq!(
                error.code,
                if possible { Code::UnknownOutcome } else { code }
            );
            assert_ne!(error.code.exit(), 0);
            assert!(HELP.contains(error.code.text()));
        }
    }
}

// Non-forwarding wire spy. It sends only a synthetic PostgreSQL authentication error;
// it cannot initialize schemas or establish TLS. No real DB, Docker, provider or DNS.
#[tokio::test]
async fn should_contact_only_selected_loopback_endpoint_and_redact_wire_failures() -> TestResult {
    for target in [Target::Business, Target::Crawler] {
        for initialize in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            listener.set_nonblocking(true)?;
            let address = listener.local_addr()?;
            let unrelated = TcpListener::bind("127.0.0.1:0")?;
            unrelated.set_nonblocking(true)?;
            let spy = std::thread::spawn(move || -> Result<(), &'static str> {
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => return Err("spy accept failed"),
                    }
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|_| "spy timeout setup failed")?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .map_err(|_| "spy timeout setup failed")?;
                let mut length = [0; 4];
                stream
                    .read_exact(&mut length)
                    .map_err(|_| "spy startup failed")?;
                let size = u32::from_be_bytes(length) as usize;
                if !(8..=4096).contains(&size) {
                    return Err("spy invalid startup size");
                }
                let mut startup = vec![0; size - 4];
                stream
                    .read_exact(&mut startup)
                    .map_err(|_| "spy startup failed")?;
                if !startup
                    .windows(b"crawler-bootstrap-local".len())
                    .any(|part| part == b"crawler-bootstrap-local")
                {
                    return Err("spy wrong application");
                }
                let payload = format!("SFATAL\0C28000\0M{SECRET}\0\0");
                let mut response = vec![b'E'];
                response.extend_from_slice(&((payload.len() + 4) as u32).to_be_bytes());
                response.extend_from_slice(payload.as_bytes());
                stream
                    .write_all(&response)
                    .map_err(|_| "spy reply failed")?;
                Ok(())
            });
            let mut values = environment(&format!(
                "postgres://private_user:password@{address}/private_db"
            ));
            let other = match target {
                Target::Business => "LOCAL_DB_URL",
                Target::Crawler => "BUSINESS_DATABASE_URL",
            };
            values.insert(
                other,
                format!(
                    "postgres://private_user:password@{}/private_db",
                    unrelated.local_addr()?
                ),
            );
            let config = load(target, initialize, &values)?;
            let mut writes = false;
            let result = if initialize {
                initialize::run(target, &config, &mut writes).await
            } else {
                verify(target, &config).await
            };
            let joined = spy.join().map_err(|_| "spy thread failed")?;
            joined?;
            assert!(!writes);
            match result {
                Err(error) => safe_chain(&error),
                Ok(()) => return Err("wire failure reported success".into()),
            }
            assert!(
                matches!(unrelated.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
            );
        }
    }
    Ok(())
}

#[test]
fn should_run_private_watchdog_child() -> TestResult {
    let Some(mode) = std::env::var_os("AURA_BOOTSTRAP_PRIVATE_WATCHDOG_CHILD") else {
        return Ok(());
    };
    watchdog(Duration::from_millis(100));
    match mode.to_str() {
        Some("panic") => {
            install_panic_hook();
            std::panic::panic_any(SECRET);
        }
        Some("output") => {
            // Parent deliberately never drains this pipe; exercise the actual output path.
            let _ = emit(Ok(&"x".repeat(1024 * 1024)), false);
        }
        Some("runtime") => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.spawn_blocking(|| std::thread::sleep(Duration::from_secs(10)));
            drop(runtime);
        }
        _ => std::thread::sleep(Duration::from_secs(10)),
    }
    Err("watchdog child unexpectedly returned".into())
}

#[cfg(unix)]
#[test]
fn should_exit_without_logging_or_core_collection_when_watchdog_deadline_expires() -> TestResult {
    use std::os::unix::process::ExitStatusExt;
    for mode in ["sleep", "output", "runtime", "panic"] {
        let mut child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "bootstrap_runtime::tests::should_run_private_watchdog_child",
                "--quiet",
                "--nocapture",
            ])
            .env_clear()
            .env("AURA_BOOTSTRAP_PRIVATE_WATCHDOG_CHILD", mode)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                child.kill()?;
                child.wait()?;
                return Err("watchdog failed to terminate child".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            pipe.read_to_string(&mut stderr)?;
        }
        assert_eq!(status.code(), Some(i32::from(Code::Runtime.exit())));
        assert!(!status.core_dumped());
        assert!(stderr.is_empty());
    }
    Ok(())
}

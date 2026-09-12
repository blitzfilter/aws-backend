//! Opt-in black-box transport proof against newly owned, local-only Docker resources.
use crate::{
    PostgresPoolConfig, PostgresTlsConfig,
    test_support::{TestDirectory, TestResult, assert_redacted_chain, generate_ca, openssl, run},
};
use sqlx::{Connection, postgres::PgSslMode};
use std::{
    error::Error,
    fs, io,
    time::{Duration, Instant},
};

#[path = "docker_fixture.rs"]
mod docker_fixture;
use docker_fixture::DockerResources;

struct PostgresFixture {
    docker: DockerResources,
    directory: TestDirectory,
    port: u16,
    root: Vec<u8>,
    password: String,
}

impl PostgresFixture {
    fn start(tls: bool, expired: bool) -> Result<Self, Box<dyn Error>> {
        let directory = TestDirectory::new()?;
        let name = directory
            .0
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("invalid fixture name")?
            .to_owned();
        let root = generate_ca(&directory, "ca")?;
        let password = String::from_utf8(run(openssl().args(["rand", "-hex", "24"]))?.stdout)?
            .trim()
            .to_owned();
        directory.file("password", password.as_bytes(), 0o600)?;
        directory.file("pg_hba.conf", if tls {
            b"local all all trust\nhostssl all all 0.0.0.0/0 scram-sha-256\nhostnossl all all 0.0.0.0/0 reject\n"
        } else {
            b"local all all trust\nhost all all 0.0.0.0/0 scram-sha-256\n"
        }, 0o644)?;
        run(openssl()
            .args([
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-noenc",
                "-subj",
                "/CN=localhost",
                "-keyout",
            ])
            .arg(directory.0.join("server.key"))
            .arg("-out")
            .arg(directory.0.join("server.csr")))?;
        directory.file("server.ext", b"basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost\n", 0o644)?;
        run(openssl()
            .args(["x509", "-req", "-in"])
            .arg(directory.0.join("server.csr"))
            .arg("-CA")
            .arg(directory.0.join("ca.crt"))
            .arg("-CAkey")
            .arg(directory.0.join("ca.key"))
            .args([
                "-CAcreateserial",
                "-days",
                if expired { "-1" } else { "1" },
                "-extfile",
            ])
            .arg(directory.0.join("server.ext"))
            .arg("-out")
            .arg(directory.0.join("server.crt")))?;
        // Guard acquires cleanup authority only after successful creation returns a full ID.
        let mut fixture = Self {
            docker: DockerResources::local(&directory.0)?,
            directory,
            port: 0,
            root,
            password,
        };
        fixture.docker.create_network(&name)?;
        // Copy into container-owned /tmp: postgres must not need access to host-owned key modes.
        let startup = "cp /policy/server.crt /tmp/server.crt && cp /policy/server.key /tmp/server.key && cp /policy/pg_hba.conf /tmp/pg_hba.conf && chown postgres:postgres /tmp/server.crt /tmp/server.key /tmp/pg_hba.conf && chmod 600 /tmp/server.key && exec docker-entrypoint.sh postgres -c ssl_cert_file=/tmp/server.crt -c ssl_key_file=/tmp/server.key -c hba_file=/tmp/pg_hba.conf";
        let startup = format!("{startup} -c ssl={}", if tls { "on" } else { "off" });
        fixture.docker.create_container(&name, |command| {
            command
                .args([
                    "--publish",
                    "127.0.0.1::5432",
                    "--tmpfs",
                    "/var/lib/postgresql/data:rw,nosuid,size=256m",
                    "--mount",
                ])
                .arg(format!(
                    "type=bind,source={},target=/policy,readonly",
                    fixture.directory.0.display()
                ))
                .args([
                    "--env",
                    "POSTGRES_PASSWORD_FILE=/policy/password",
                    "--env",
                    "POSTGRES_USER=policy_user",
                    "--env",
                    "POSTGRES_DB=policy_db",
                    "--entrypoint",
                    "/bin/sh",
                    "postgres:17",
                    "-c",
                ])
                .arg(startup);
        })?;
        fixture.docker.start_container()?;

        let output = run(fixture
            .docker
            .command()
            .args([
                "inspect",
                "--format",
                "{{(index (index .NetworkSettings.Ports \"5432/tcp\") 0).HostPort}}",
            ])
            .arg(fixture.docker.container_id()?))?;
        fixture.port = String::from_utf8(output.stdout)?.trim().parse()?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let ready = fixture
                .docker
                .command()
                .args(["exec"])
                .arg(fixture.docker.container_id()?)
                .args([
                    "pg_isready",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    "policy_user",
                    "-d",
                    "policy_db",
                ])
                .output()?;
            if ready.status.success() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(
                    "isolated PostgreSQL startup timed out (logs suppressed)",
                )
                .into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Ok(fixture)
    }

    fn close(mut self) -> io::Result<()> {
        self.docker.cleanup()
    }

    fn config(
        &self,
        host: &str,
        root: Vec<u8>,
        password: &str,
    ) -> Result<PostgresPoolConfig, crate::PostgresPoolConfigError> {
        let tls = PostgresTlsConfig::new("prod", "verify-full", Some(root), "policy-tls-test")?;
        PostgresPoolConfig::new(
            host.into(),
            self.port,
            "policy_db".into(),
            "policy_user".into(),
            password.into(),
            1,
            tls,
        )
    }
}

#[tokio::test]
#[ignore = "requires local Docker postgres:17 image, openssl and timeout; never pulls an image"]
async fn should_connect_session_with_verified_tls_and_redact_rejected_connections() -> TestResult {
    let fixture = PostgresFixture::start(true, false)?;
    let config = fixture.config("localhost", fixture.root.clone(), &fixture.password)?;
    let mut session = config.connect_session().await?;
    let encrypted: bool =
        sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
            .fetch_one(&mut session)
            .await
            .map_err(|_| "session TLS query failed (provider output suppressed)")?;
    assert!(encrypted);
    let app: String = sqlx::query_scalar("SELECT current_setting('application_name')")
        .fetch_one(&mut session)
        .await
        .map_err(|_| "session application query failed (provider output suppressed)")?;
    assert_eq!(app, "policy-tls-test");
    let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut session)
        .await
        .map_err(|_| "session identity query failed (provider output suppressed)")?;
    // Dedicated sessions are independent, even when the configured pool cap is one.
    let mut other = config.connect_session().await?;
    let other_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut other)
        .await
        .map_err(|_| "session identity query failed (provider output suppressed)")?;
    assert_ne!(first_pid, other_pid);
    session
        .close()
        .await
        .map_err(|_| "session close failed (provider output suppressed)")?;
    other
        .close()
        .await
        .map_err(|_| "session close failed (provider output suppressed)")?;

    let wrong_ca = generate_ca(&fixture.directory, "untrusted-session")?;
    let wrong_password = "incorrect-session-password-canary";
    let rejected = [
        fixture.config("127.0.0.1", fixture.root.clone(), &fixture.password)?,
        fixture.config("localhost", wrong_ca, &fixture.password)?,
        fixture.config("localhost", fixture.root.clone(), wrong_password)?,
    ];
    for config in rejected {
        let error = config
            .connect_session()
            .await
            .err()
            .ok_or("invalid session connection accepted")?;
        assert_redacted_chain(
            &error,
            &[
                "policy_user",
                &fixture.password,
                wrong_password,
                "password authentication failed for user",
            ],
        );
    }
    fixture.close()?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires local Docker postgres:17 image, openssl and timeout; never pulls an image"]
async fn should_enforce_authenticated_tls_against_isolated_postgres() -> TestResult {
    let fixture = PostgresFixture::start(true, false)?;
    let config = fixture.config("localhost", fixture.root.clone(), &fixture.password)?;
    assert!(matches!(
        config.connect_options().get_ssl_mode(),
        PgSslMode::VerifyFull
    ));
    let pool = config.connect().await?;
    let encrypted: bool =
        sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .map_err(|_| "TLS state query failed (provider output suppressed)")?;
    assert!(encrypted);
    let app: String = sqlx::query_scalar("SELECT current_setting('application_name')")
        .fetch_one(&pool)
        .await
        .map_err(|_| "application name query failed (provider output suppressed)")?;
    assert_eq!(app, "policy-tls-test");
    pool.close().await;

    // The URL path must use the same protected policy on a real TLS connection.
    let tls = PostgresTlsConfig::new(
        "prod",
        "verify-full",
        Some(fixture.root.clone()),
        "policy-tls-test",
    )?;
    let url = format!(
        "postgres://policy_user:{}@localhost:{}/policy_db?sslmode=verify-full",
        fixture.password, fixture.port
    );
    let pool = PostgresPoolConfig::from_url(&url, 1, tls)?
        .connect()
        .await?;
    pool.close().await;

    let wrong_host = fixture
        .config("127.0.0.1", fixture.root.clone(), &fixture.password)?
        .connect()
        .await;
    assert!(wrong_host.is_err());
    let wrong_ca = generate_ca(&fixture.directory, "untrusted")?;
    assert!(
        fixture
            .config("localhost", wrong_ca, &fixture.password)?
            .connect()
            .await
            .is_err()
    );
    let wrong_password = "incorrect-password-private-canary";
    let error = fixture
        .config("localhost", fixture.root.clone(), wrong_password)?
        .connect()
        .await
        .err()
        .ok_or("wrong password accepted")?;
    assert_redacted_chain(
        &error,
        &[
            "policy_user",
            &fixture.password,
            wrong_password,
            "password authentication failed for user",
        ],
    );
    let source = error.source().ok_or("classified cause not retained")?;
    assert_eq!(source.to_string(), "PostgreSQL authentication rejected");
    assert!(source.source().is_none());

    let local = PostgresTlsConfig::new("test", "disable", None, "policy-tls-test")?;
    let plaintext = PostgresPoolConfig::new(
        "localhost".into(),
        fixture.port,
        "policy_db".into(),
        "policy_user".into(),
        fixture.password.clone(),
        1,
        local,
    )?;
    assert!(plaintext.connect().await.is_err());
    fixture.close()?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires local Docker postgres:17 image, openssl and timeout; never pulls an image"]
async fn should_reject_plaintext_server_but_allow_explicit_test_policy() -> TestResult {
    let fixture = PostgresFixture::start(false, false)?;
    assert!(
        fixture
            .config("localhost", fixture.root.clone(), &fixture.password)?
            .connect()
            .await
            .is_err()
    );
    let local = PostgresTlsConfig::new("test", "disable", None, "policy-tls-test")?;
    let config = PostgresPoolConfig::new(
        "localhost".into(),
        fixture.port,
        "policy_db".into(),
        "policy_user".into(),
        fixture.password.clone(),
        1,
        local,
    )?;
    let pool = config.connect().await?;
    let encrypted: bool =
        sqlx::query_scalar("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")
            .fetch_one(&pool)
            .await
            .map_err(|_| "TLS state query failed (provider output suppressed)")?;
    assert!(!encrypted);
    pool.close().await;
    fixture.close()?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires local Docker postgres:17 image, openssl and timeout; never pulls an image"]
async fn should_reject_expired_server_certificate() -> TestResult {
    let fixture = PostgresFixture::start(true, true)?;
    let certificate = fs::read(fixture.directory.0.join("server.crt"))?;
    assert!(!certificate.is_empty());
    assert!(
        fixture
            .config("localhost", fixture.root.clone(), &fixture.password)?
            .connect()
            .await
            .is_err()
    );
    fixture.close()?;
    Ok(())
}

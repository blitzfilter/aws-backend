//! Opt-in real local Docker tests. No LocalStack, Sequin process, or worker is started.
use super::*;
use crate::{IntegrationTestService, Postgres, get_postgres_client, postgres as pg};
use sqlx::{AssertSqlSafe, Executor};
use std::{io::Write, task::Poll, time::Duration};

fn fixture_image() -> Result<String, std::env::VarError> {
    match std::env::var("AURA_TEST_POSTGRES_IMAGE") {
        Ok(image) => Ok(image),
        Err(std::env::VarError::NotPresent) => Ok(pg::POSTGRES_PG_TTL_IMAGE.trim().into()),
        Err(error) => Err(error),
    }
}

fn record_acquired_id(id: &str) -> TestResult {
    let path = std::env::var_os("AURA_POSTGRES_FIXTURE_ID_RECORD")
        .ok_or("owned subprocess ID record missing")?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{id}")?;
    file.sync_all()?;
    Ok(())
}

fn process_owned_id() -> Result<String, Box<dyn Error + Send + Sync>> {
    let owned = pg::OWNED_CONTAINER
        .lock()
        .map_err(|_| "ownership lock poisoned")?;
    Ok(owned
        .as_ref()
        .ok_or("no process-owned container")?
        .id()?
        .to_owned())
}

#[test]
#[ignore = "requires local Unix Docker and a cached pinned/explicit local pg_ttl image; never pulls"]
fn should_start_postgres_and_verify_owned_cleanup_in_subprocesses() -> TestResult {
    let image = fixture_image().map_err(|_| "invalid fixture image override")?;
    let verifier = DockerFixture::local()?;
    checked(
        verifier
            .command()
            .args(["image", "inspect", "--format", "{{.Id}}", &image]),
    )
    .map_err(|_| "local cached fixture image unavailable; no pull attempted (output suppressed)")?;
    for (mode, count, exit) in [
        ("exit", 2, 0),
        ("collision", 1, 0),
        ("partial", 1, 0),
        ("signal", 1, 1),
    ] {
        let record = verifier.config_dir.join(format!("{mode}-ids"));
        let output = child_command(
            "postgres::fixture::tests::process_tests::should_run_owned_postgres_child",
        )?
        .env("AURA_POSTGRES_FIXTURE_CHILD", mode)
        .env("AURA_POSTGRES_FIXTURE_ID_RECORD", &record)
        .env("AURA_TEST_POSTGRES_IMAGE", &image)
        .output()?;
        let ids = fs::read_to_string(record)
            .map_err(|_| "child did not record acquired ownership; output suppressed")?;
        let mut leaked = false;
        for id in ids.lines() {
            if id.len() != 64
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("invalid subprocess ownership record; no cleanup authority".into());
            }
            // Authority transfers only through this private record written after successful create
            // by our exact child binary. Neither Docker lookups nor names authorize recovery.
            let mut recovery = DockerFixture::local()?;
            recovery.container = Some(ContainerId(id.to_owned()));
            leaked |= recovery.exists(id)?;
            recovery.cleanup()?;
            assert!(!verifier.exists(id)?);
            println!("{mode}: verified removed owned container {id}");
        }
        assert!(
            !leaked,
            "child leaked an owned container; ID-only recovery removed it"
        );
        assert_eq!(
            ids.lines().count(),
            count,
            "unexpected owned container count"
        );
        assert_eq!(
            output.status.code(),
            Some(exit),
            "fixture child failed (output suppressed)"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "isolated subprocess entry; run through should_start_postgres_and_verify_owned_cleanup_in_subprocesses"]
async fn should_run_owned_postgres_child() -> TestResult {
    assert_eq!(std::env::var("DOCKER_HOST")?, "tcp://remote.invalid:2376");
    pg::cleanup_owned_container()?;
    let mode = std::env::var("AURA_POSTGRES_FIXTURE_CHILD")?;
    let image = fixture_image().map_err(|_| "invalid fixture image override")?;
    if mode == "collision" {
        let mut original = DockerFixture::local()?;
        original.create(&pg::postgres_container_name(), &image)?;
        let id = original.id()?.to_owned();
        record_acquired_id(&id)?;
        assert!(pg::start_container().await.is_err());
        assert!(original.exists(&id)?);
        assert!(
            pg::OWNED_CONTAINER
                .lock()
                .map_err(|_| "ownership lock poisoned")?
                .is_none()
        );
        original.cleanup()?;
        assert!(!original.exists(&id)?);
        return Ok(());
    }
    if mode == "partial" {
        let mut startup = Box::pin(pg::start_container());
        // Poll through create/start to the first SQL await, then cancel the real initializer.
        std::future::poll_fn(|cx| match startup.as_mut().poll(cx) {
            Poll::Pending => Poll::Ready(Ok(())),
            Poll::Ready(_) => Poll::Ready(Err("startup completed before cancellation boundary")),
        })
        .await?;
        let id = process_owned_id()?;
        record_acquired_id(&id)?;
        drop(startup);
        assert!(
            pg::OWNED_CONTAINER
                .lock()
                .map_err(|_| "ownership lock poisoned")?
                .is_none()
        );
        assert!(!DockerFixture::local()?.exists(&id)?);
        return Ok(());
    }

    let service = Postgres::with_setup_script(
        "src/test-api/tests/fixtures/rds_migrations",
        "src/test-api/tests/fixtures/postgres_setup.sql",
    );
    service.set_up().await;
    record_acquired_id(&process_owned_id()?)?;
    let pool = get_postgres_client().await;
    let configured: bool = sqlx::query_scalar(AssertSqlSafe(
        "SELECT current_setting('wal_level') = 'logical'
         AND current_setting('shared_preload_libraries') LIKE '%pg_ttl_index%'
         AND EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_ttl_index')",
    ))
    .fetch_one(&pool)
    .await
    .map_err(|_| "fixture pg_ttl verification failed")?;
    assert!(configured);
    if mode == "signal" {
        tokio::time::timeout(Duration::from_secs(5), pool.close()).await?;
        checked(
            Command::new("/usr/bin/timeout")
                .args([
                    "--kill-after=2s",
                    "5s",
                    "/usr/bin/kill",
                    "-TERM",
                    &std::process::id().to_string(),
                ])
                .env_clear()
                .env("PATH", "/usr/bin:/bin"),
        )?;
        tokio::time::sleep(Duration::from_secs(75)).await;
        return Err("owned Postgres signal cleanup did not exit".into());
    }
    assert_eq!(mode, "exit");
    for iteration in 0..2 {
        if iteration != 0 {
            service.set_up().await;
        }
        let count: i64 = sqlx::query_scalar(AssertSqlSafe(
            "SELECT count(*) FROM test_items WHERE name = 'from-setup-script'",
        ))
        .fetch_one(&pool)
        .await
        .map_err(|_| "raw fixture replay query failed")?;
        assert_eq!(count, 1);
        let ledger_absent: bool = sqlx::query_scalar(AssertSqlSafe(
            "SELECT to_regclass('public._sqlx_migrations') IS NULL",
        ))
        .fetch_one(&pool)
        .await
        .map_err(|_| "raw fixture ledger check failed")?;
        assert!(
            ledger_absent,
            "raw seed fixture must not stamp SQLx history"
        );
        service.tear_down().await;
        let count: i64 = sqlx::query_scalar(AssertSqlSafe("SELECT count(*) FROM test_items"))
            .fetch_one(&pool)
            .await
            .map_err(|_| "fixture truncation check failed")?;
        assert_eq!(count, 0);
        if iteration == 0 {
            pool.execute(AssertSqlSafe("DROP TABLE test_tags, test_items"))
                .await
                .map_err(|_| "owned fixture replay preparation failed")?;
        }
    }

    // Model Sequin's separate-container host-gateway access without starting Sequin.
    let mut peer = DockerFixture::local()?;
    peer.container = Some(ContainerId::created(
        peer.command()
            .args([
                "create",
                "--pull=never",
                "--name",
                &format!("aura-postgres-gateway-{}", uuid::Uuid::new_v4()),
                "--add-host",
                "host.docker.internal:host-gateway",
                "--tmpfs",
                "/var/lib/postgresql/data:rw,nosuid",
                &image,
                "sleep",
                "90",
            ])
            .output()?,
    )?);
    let peer_id = peer.id()?.to_owned();
    record_acquired_id(&peer_id)?;
    checked(peer.command().args(["start", &peer_id]))?;
    let output = checked(peer.command().args([
        "exec",
        "--env",
        "PGPASSWORD=postgres",
        &peer_id,
        "psql",
        "-h",
        "host.docker.internal",
        "-p",
        &pg::postgres_host_port().to_string(),
        "-U",
        "postgres",
        "-d",
        "postgres",
        "-tAc",
        "SELECT 1",
    ]))?;
    assert_eq!(output.stdout.trim_ascii(), b"1");
    assert!(
        pg::get_postgres_host_gateway_connection_string("postgres")
            .contains("@host.docker.internal:")
    );
    peer.cleanup()?;
    assert!(!peer.exists(&peer_id)?);
    tokio::time::timeout(Duration::from_secs(5), pool.close()).await?;
    // Leave the process-lived fixture to the actual atexit hook. Parent verifies its exact ID.
    Ok(())
}

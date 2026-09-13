use super::{
    error::{TestError, TestResult},
    process::{CleanupMode, run_process},
};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::Duration,
};
pub(super) const IMAGE: &str = include_str!("../../../test-api/postgres/image-ref.txt");
pub(super) const CHILD_ENTRY: &str = "should_run_owned_postgres_preflight_child";

pub(super) struct Directory(pub PathBuf);
impl Directory {
    pub fn new() -> TestResult<Self> {
        let path = Path::new("/tmp").join(format!("crawler-preflight-{}", uuid::Uuid::now_v7()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }

    pub fn close(mut self) -> TestResult {
        fs::remove_dir_all(&self.0)?;
        assert!(!self.0.try_exists()?, "owned directory survived cleanup");
        self.0.clear();
        Ok(())
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty()
            && let Err(error) = fs::remove_dir_all(&self.0)
        {
            eprintln!("owned preflight directory cleanup failed: {}", error.kind());
        }
    }
}

/// Inspection only. Container creation/removal stays exclusively inside test-api.
/// An observed ID is evidence, never authority for recovery or deletion.
fn docker(directory: &Path) -> Command {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args([
            "--signal=TERM",
            "--kill-after=2s",
            "20s",
            "/usr/bin/docker",
            "--host",
            "unix:///var/run/docker.sock",
            "--config",
        ])
        .arg(directory)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null());
    command
}

fn checked(command: &mut Command) -> TestResult<Output> {
    let output = run_process(command, Duration::from_secs(22), CleanupMode::Kill)?;
    if !output.status.success() {
        return Err(TestError::failure("LOCAL_DOCKER_INSPECTION"));
    }
    Ok(output)
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn observe_started_fixture(directory: &Path, port: u16) -> TestResult {
    // Only after get_postgres_client succeeded. A name collision fails before here.
    // This lookup does NOT transfer test-api's successful-create cleanup authority.
    let output = checked(docker(directory).args([
        "inspect",
        "--format",
        "{{.Id}} {{(index (index .NetworkSettings.Ports \"5432/tcp\") 0).HostPort}}",
        &format!(
            "aura-historia-aws-backend-postgres-test-{}",
            std::process::id()
        ),
    ]))?;
    let text = std::str::from_utf8(&output.stdout)?;
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.len() != 2 || !valid_id(fields[0]) || fields[1].parse::<u16>()? != port {
        return Err("fixture observation does not match its returned endpoint".into());
    }
    fs::write(directory.join("observed-container-id"), fields[0])?;
    Ok(())
}

fn verify_fixture_absent(directory: &Path) -> TestResult {
    let id = fs::read_to_string(directory.join("observed-container-id"))?;
    if !valid_id(&id) {
        return Err("invalid observation record; no container operation authorized".into());
    }
    let output = checked(docker(directory).args([
        "container",
        "ls",
        "--all",
        "--no-trunc",
        "--quiet",
        "--filter",
        &format!("id={id}"),
    ]))?;
    if !output.stdout.trim_ascii().is_empty() {
        eprintln!(
            "cleanup failed for observed fixture ID {id}; only test-api owns deletion authority"
        );
        return Err(TestError::failure("FIXTURE_CONTAINER_PRESENT"));
    }
    println!("cleanup: verified absent exact fixture container ID {id}");
    Ok(())
}

pub(super) fn supervise_fixture() -> TestResult {
    if std::env::var("AURA_CRAWLER_ISOLATED_LOCAL_POSTGRES").as_deref() != Ok("1") {
        return Err("requires AURA_CRAWLER_ISOLATED_LOCAL_POSTGRES=1 on an isolated local coding host; test-api publishes 0.0.0.0".into());
    }
    let directory = Directory::new()?;
    let result = (|| {
        checked(docker(&directory.0).args([
            "image",
            "inspect",
            "--format",
            "{{.Id}}",
            IMAGE.trim(),
        ]))
        .map_err(|error| error.context("CACHED_IMAGE_UNAVAILABLE_NO_PULL"))?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args([
                CHILD_ENTRY,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &directory.0)
            .env("AURA_CRAWLER_PREFLIGHT_PARENT", &directory.0)
            .env("AURA_TEST_POSTGRES_IMAGE", IMAGE.trim());
        // Leave Docker inspection and hook grace inside the caller's 240-second budget.
        let result = run_process(
            &mut command,
            Duration::from_secs(110),
            CleanupMode::FixtureHooks,
        );
        // Check hooks after process exit, even when the assertion suite failed.
        let absent = verify_fixture_absent(&directory.0);
        if let Ok(output) = &result {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            // Never relay child logs/panic text. Reconstruct only known case IDs and classes.
            let mut reports = fixture_reports(&text);
            reports.extend(super::idle_daemon::reports(&text));
            reports.extend(super::fresh_bootstrap::reports(&text));
            for report in &reports {
                println!("{report}");
            }
            for evidence in super::fresh_bootstrap::evidence_reports(&text) {
                println!("{evidence}");
            }
            if text.contains("cleanup failed") {
                absent?;
                return Err(TestError::failure("FIXTURE_HOOK_CLEANUP"));
            }
            if !reports.iter().all(|report| report.starts_with("PASS ")) {
                absent?;
                return Err(TestError::failure("FIXTURE_CASE_REPORTS"));
            }
            if output.status.success() {
                println!("PASS all {} actual-server PostgreSQL cases", reports.len());
            }
        }
        absent?;
        if !result?.status.success() {
            return Err(TestError::failure("FIXTURE_CHILD_EXIT"));
        }
        Ok(())
    })();
    directory.close()?;
    result
}

pub(super) fn fixture_reports(text: &str) -> Vec<String> {
    super::cases()
        .into_iter()
        .map(|case| {
            let pass = case.pass_report();
            let failure = format!("FAIL {case:?}: ");
            if let Some(kind) = text.lines().find_map(|line| line.strip_prefix(&failure)) {
                format!("{failure}{}", TestError::report_kind(kind))
            } else if text.lines().any(|line| line == pass) {
                pass
            } else {
                format!("UNREPORTED {case:?}")
            }
        })
        .collect()
}

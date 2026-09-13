use super::*;
use rstest::rstest;
use std::{error::Error, os::unix::fs::PermissionsExt, time::Instant};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
const OWNED: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FOREIGN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NAME: &str = "colliding-postgres-name";
const IMAGE: &str = "intentional-local-postgres:test";

struct FakeDocker(PathBuf);

impl FakeDocker {
    fn new() -> io::Result<Self> {
        let fake = Self(create_config_directory()?);
        let program = fake.0.join("docker");
        fs::write(
            &program,
            br#"#!/usr/bin/python3
import json, os, pathlib, signal, sys, time
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
with (root / 'calls').open('a') as log:
    log.write(json.dumps(args) + '\n')
(root / 'environment').write_text(json.dumps(dict(os.environ)))
op = args[4:]
step = 'list' if op[0] == 'container' else op[0]
if (root / ('hang-' + step)).exists():
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    time.sleep(90)
code, stdout = (root / ('reply-' + step)).read_bytes().split(b'\n', 1)
code = int(code)
if not code and step == 'create' and len(stdout.strip()) == 64:
    (root / 'alive').write_bytes(stdout.strip())
if not code and step == 'rm':
    assert op == ['rm', '--force', (root / 'alive').read_text()]
    if not (root / 'keep-alive').exists():
        (root / 'alive').unlink()
if step == 'list' and not code:
    stdout = (root / 'alive').read_bytes() if (root / 'alive').exists() else b''
sys.stdout.buffer.write(stdout)
if code:
    sys.stderr.write('private-provider-body-canary')
sys.exit(code)
"#,
        )?;
        fs::set_permissions(program, fs::Permissions::from_mode(0o700))?;
        for (step, output) in [
            ("create", OWNED),
            ("start", OWNED),
            ("inspect", "15432"),
            ("rm", ""),
            ("list", ""),
        ] {
            fake.reply(step, 0, output.as_bytes())?;
        }
        Ok(fake)
    }

    fn reply(&self, step: &str, code: u8, stdout: &[u8]) -> io::Result<()> {
        let mut reply = format!("{code}\n").into_bytes();
        reply.extend_from_slice(stdout);
        fs::write(self.0.join(format!("reply-{step}")), reply)
    }

    fn fixture(&self) -> io::Result<DockerFixture> {
        Ok(DockerFixture {
            program: self.0.join("docker"),
            config_dir: create_config_directory()?,
            container: None,
        })
    }

    fn operations(&self) -> Result<Vec<Vec<String>>, Box<dyn Error + Send + Sync>> {
        let calls = match fs::read_to_string(self.0.join("calls")) {
            Ok(calls) => calls,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        calls
            .lines()
            .map(|line| {
                let args: Vec<String> = serde_json::from_str(line)?;
                assert_eq!(&args[..3], ["--host", LOCAL_ENDPOINT, "--config"]);
                assert!(args[3].starts_with("/tmp/aura-test-postgres-"));
                Ok(args[4..].to_vec())
            })
            .collect()
    }

    fn removals(&self) -> Result<Vec<Vec<String>>, Box<dyn Error + Send + Sync>> {
        Ok(self
            .operations()?
            .into_iter()
            .filter(|op| op[0] == "rm")
            .collect())
    }
}

impl Drop for FakeDocker {
    fn drop(&mut self) {
        if fs::remove_dir_all(&self.0).is_err() {
            eprintln!("owned fake Docker directory cleanup failed");
        }
    }
}

fn assert_redacted(error: &io::Error) {
    assert!(!format!("{error:?} {error}").contains("private-provider-body-canary"));
}

#[test]
fn should_do_no_docker_work_when_cleanup_precedes_successful_creation() -> TestResult {
    let fake = FakeDocker::new()?;
    {
        let mut fixture = fake.fixture()?;
        fixture.cleanup()?;
        assert!(fixture.start().is_err());
    }
    assert!(fake.operations()?.is_empty());
    Ok(())
}

#[test]
fn should_not_delete_unrelated_container_when_name_collides() -> TestResult {
    let fake = FakeDocker::new()?;
    fake.reply("create", 1, FOREIGN.as_bytes())?;
    fs::write(fake.0.join("alive"), FOREIGN)?;
    {
        let mut fixture = fake.fixture()?;
        let error = fixture
            .create(NAME, IMAGE)
            .err()
            .ok_or("collision accepted")?;
        assert_redacted(&error);
        fixture.cleanup()?;
    }
    assert_eq!(fake.operations()?.len(), 1);
    assert!(fake.removals()?.is_empty());
    assert_eq!(fs::read_to_string(fake.0.join("alive"))?, FOREIGN);
    Ok(())
}

#[rstest]
#[case(b"".to_vec())]
#[case(b" \n".to_vec())]
#[case(b"short".to_vec())]
#[case(b"--all".to_vec())]
#[case(vec![b'a'; 63])]
#[case(vec![b'a'; 65])]
#[case(vec![b'A'; 64])]
#[case(vec![b'g'; 64])]
#[case(vec![0xff; 64])]
#[case(format!("{OWNED}\n{FOREIGN}").into_bytes())]
fn should_acquire_no_cleanup_authority_when_create_id_is_invalid(
    #[case] id: Vec<u8>,
) -> TestResult {
    let fake = FakeDocker::new()?;
    fake.reply("create", 0, &id)?;
    {
        let mut fixture = fake.fixture()?;
        assert!(fixture.create(NAME, IMAGE).is_err());
        fixture.cleanup()?;
    }
    assert_eq!(fake.operations()?.len(), 1);
    assert!(fake.removals()?.is_empty());
    Ok(())
}

#[rstest]
#[case("start", 1, FOREIGN)]
#[case("inspect", 1, FOREIGN)]
#[case("inspect", 0, "")]
#[case("inspect", 0, "0")]
#[case("inspect", 0, "65536")]
#[case("inspect", 0, "15432\n15433")]
fn should_remove_exact_owned_id_when_partial_start_fails(
    #[case] step: &str,
    #[case] status: u8,
    #[case] output: &str,
) -> TestResult {
    let fake = FakeDocker::new()?;
    fake.reply(step, status, output.as_bytes())?;
    {
        let mut fixture = fake.fixture()?;
        fixture.create(NAME, IMAGE)?;
        assert_redacted(&fixture.start().err().ok_or("partial failure accepted")?);
    }
    assert_eq!(fake.removals()?, vec![vec!["rm", "--force", OWNED]]);
    assert!(!fake.0.join("alive").exists());
    Ok(())
}

#[test]
fn should_remove_once_and_verify_absence_without_deleting_volumes() -> TestResult {
    let fake = FakeDocker::new()?;
    let mut fixture = fake.fixture()?;
    fixture.create(NAME, IMAGE)?;
    assert_eq!(fixture.start()?, 15432);
    assert!(fixture.create("another-name", IMAGE).is_err());
    fixture.cleanup()?;
    fixture.cleanup()?;
    let config = fixture.config_dir.clone();
    drop(fixture);
    assert!(!config.exists());
    assert_eq!(fake.removals()?, vec![vec!["rm", "--force", OWNED]]);
    let calls = fake.operations()?;
    assert_eq!(calls.len(), 6);
    assert_eq!(
        calls[0][..5],
        ["create", "--pull=never", "--name", NAME, "--publish"]
    );
    assert_eq!(calls[0][5], "0.0.0.0::5432");
    assert!(calls[0].iter().any(|arg| arg == IMAGE));
    assert!(calls[0].iter().any(|arg| arg == "wal_level=logical"));
    assert!(
        calls[0]
            .iter()
            .any(|arg| arg == "shared_preload_libraries=pg_ttl_index")
    );
    assert!(
        calls
            .iter()
            .flatten()
            .all(|arg| !matches!(arg.as_str(), "--volumes" | "-v" | "prune" | "pull"))
    );
    assert_eq!(
        calls[5],
        vec![
            "container",
            "ls",
            "--all",
            "--no-trunc",
            "--quiet",
            "--filter",
            &format!("id={OWNED}")
        ]
    );
    Ok(())
}

#[rstest]
#[case("rm")]
#[case("list")]
fn should_report_cleanup_failure_and_retain_only_owned_id_for_retry(
    #[case] step: &str,
) -> TestResult {
    let fake = FakeDocker::new()?;
    let mut fixture = fake.fixture()?;
    fixture.create(NAME, IMAGE)?;
    fake.reply(step, 1, FOREIGN.as_bytes())?;
    assert_redacted(&fixture.cleanup().err().ok_or("cleanup failure ignored")?);
    assert_eq!(fixture.id()?, OWNED);
    fake.reply(step, 0, b"")?;
    fixture.cleanup()?;
    assert!(fixture.container.is_none());
    assert!(!fake.0.join("alive").exists());
    Ok(())
}

#[test]
fn should_fail_cleanup_when_successful_remove_does_not_remove_container() -> TestResult {
    let fake = FakeDocker::new()?;
    let mut fixture = fake.fixture()?;
    fixture.create(NAME, IMAGE)?;
    fs::write(fake.0.join("keep-alive"), "")?;
    assert!(fixture.cleanup().is_err());
    assert_eq!(fixture.id()?, OWNED);
    fs::remove_file(fake.0.join("keep-alive"))?;
    fixture.cleanup()?;
    Ok(())
}

#[test]
fn should_bound_cleanup_even_when_docker_ignores_sigterm() -> TestResult {
    let fake = FakeDocker::new()?;
    let mut fixture = fake.fixture()?;
    fixture.create(NAME, IMAGE)?;
    fs::write(fake.0.join("hang-rm"), "")?;
    let started = Instant::now();
    assert!(fixture.cleanup().is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(35));
    fs::remove_file(fake.0.join("hang-rm"))?;
    fixture.cleanup()?;
    Ok(())
}

#[test]
fn should_reject_options_as_image_without_any_docker_command() -> TestResult {
    let fake = FakeDocker::new()?;
    let mut fixture = fake.fixture()?;
    for image in ["", "--privileged", "image\n--network=host", "image\0secret"] {
        assert!(fixture.create(NAME, image).is_err());
    }
    assert!(fake.operations()?.is_empty());
    Ok(())
}

#[test]
fn should_require_a_unix_socket_not_a_file_directory_or_missing_path() -> TestResult {
    let fake = FakeDocker::new()?;
    assert!(require_local_socket(&fake.0).is_err());
    assert!(require_local_socket(&fake.0.join("missing")).is_err());
    assert!(require_local_socket(&fake.0.join("docker")).is_err());
    let path = fake.0.join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&path)?;
    require_local_socket(&path)?;
    Ok(())
}

fn child_command(test: &str) -> io::Result<Command> {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args(["--signal=TERM", "--kill-after=70s", "120s"])
        .arg(std::env::current_exe()?)
        .args([
            "--exact",
            test,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    poison_environment(&mut command);
    Ok(command)
}

fn poison_environment(command: &mut Command) {
    for (key, value) in [
        ("DOCKER_HOST", "tcp://remote.invalid:2376"),
        ("DOCKER_CONTEXT", "remote-context"),
        ("DOCKER_CONFIG", "/untrusted/docker-config"),
        ("DOCKER_TLS", "1"),
        ("DOCKER_TLS_VERIFY", "1"),
        ("DOCKER_CERT_PATH", "/untrusted/certificates"),
        ("DOCKER_CERT_CONFIG", "/untrusted/cert-config"),
        ("DOCKER_API_VERSION", "private-provider-body-canary"),
        ("HOME", "/untrusted/home"),
        ("HTTP_PROXY", "http://remote.invalid:9999"),
    ] {
        command.env(key, value);
    }
}

#[test]
fn should_pin_local_argv_and_environment_when_parent_is_poisoned() -> TestResult {
    checked(&mut child_command(
        "postgres::fixture::tests::should_check_poisoned_environment_in_child",
    )?)?;
    Ok(())
}

#[test]
#[ignore = "subprocess entry; run through should_pin_local_argv_and_environment_when_parent_is_poisoned"]
fn should_check_poisoned_environment_in_child() -> TestResult {
    assert_eq!(std::env::var("DOCKER_HOST")?, "tcp://remote.invalid:2376");
    let fake = FakeDocker::new()?;
    let mut fixture = fake.fixture()?;
    let command = fixture.command();
    assert_eq!(command.get_program(), "/usr/bin/timeout");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(&args[..3], ["--signal=TERM", "--kill-after=2s", "20s"]);
    assert_eq!(args[3], fixture.program);
    assert_eq!(&args[4..7], ["--host", LOCAL_ENDPOINT, "--config"]);
    assert_eq!(args[7], fixture.config_dir);
    let env: Vec<_> = command.get_envs().collect();
    assert_eq!(
        env,
        vec![(
            std::ffi::OsStr::new("PATH"),
            Some(std::ffi::OsStr::new("/usr/bin:/bin"))
        )]
    );
    fixture.create(NAME, IMAGE)?;
    fixture.start()?;
    fixture.cleanup()?;
    fake.operations()?;
    let environment: std::collections::HashMap<String, String> =
        serde_json::from_slice(&fs::read(fake.0.join("environment"))?)?;
    // Python may inject LC_CTYPE; no Docker, HOME, proxy, or certificate settings survive.
    assert!(
        environment
            .keys()
            .all(|key| matches!(key.as_str(), "PATH" | "LC_CTYPE"))
    );
    Ok(())
}

#[path = "postgres_fixture_process_tests.rs"]
mod process_tests;

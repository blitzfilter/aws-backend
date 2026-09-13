use super::*;
use crate::test_support::{TestDirectory, TestResult};

const NETWORK: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CONTAINER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const FOREIGN: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const NETWORK_NAME: &str = "colliding-network-name";
const CONTAINER_NAME: &str = "colliding-container-name";

struct FakeDocker(TestDirectory);

impl FakeDocker {
    fn new() -> io::Result<Self> {
        let fake = Self(TestDirectory::new()?);
        fake.0.file(
            "fake-docker",
            br#"#!/usr/bin/python3
import os, pathlib, sys
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
with (root / 'calls').open('a') as log:
    log.write('\0'.join(args) + '\n')
with (root / 'environment').open('w') as log:
    log.write('\n'.join(sorted(key for key in os.environ if key.startswith('DOCKER_'))))
op = args[4:]
if op[:2] == ['network', 'create']:
    reply = 'network'
elif op[:1] == ['create']:
    reply = 'container'
elif op[:1] == ['start']:
    reply = 'start'
elif op[:1] == ['rm']:
    reply = 'remove-container'
elif op[:2] == ['network', 'rm']:
    reply = 'remove-network'
else:
    sys.exit(99)
code, stdout = (root / reply).read_text().split('\n', 1)
sys.stdout.write(stdout)
if int(code):
    sys.stderr.write('private-provider-body-canary')
sys.exit(int(code))
"#,
            0o700,
        )?;
        for (step, output) in [
            ("network", NETWORK),
            ("container", CONTAINER),
            ("start", CONTAINER),
            ("remove-container", ""),
            ("remove-network", ""),
        ] {
            fake.reply(step, 0, output)?;
        }
        fs::create_dir(fake.0.0.join("docker-config"))?;
        Ok(fake)
    }

    fn reply(&self, step: &str, status: u8, output: &str) -> io::Result<()> {
        self.0
            .file(step, format!("{status}\n{output}\n").as_bytes(), 0o600)?;
        Ok(())
    }

    fn resources(&self) -> DockerResources {
        DockerResources {
            program: self.0.0.join("fake-docker"),
            config_dir: self.0.0.join("docker-config"),
            network: None,
            container: None,
        }
    }

    fn operations(&self) -> io::Result<Vec<Vec<String>>> {
        let calls = fs::read_to_string(self.0.0.join("calls"))?;
        calls
            .lines()
            .map(|line| {
                let args: Vec<String> = line.split('\0').map(str::to_owned).collect();
                assert!(args.len() > 4);
                assert_eq!(args[0], "--host");
                assert_eq!(args[1], LOCAL_ENDPOINT);
                assert_eq!(args[2], "--config");
                assert!(Path::new(&args[3]) == self.0.0.join("docker-config"));
                Ok(args[4..].to_vec())
            })
            .collect()
    }

    fn removals(&self) -> io::Result<Vec<Vec<String>>> {
        Ok(self
            .operations()?
            .into_iter()
            .filter(|op| op[0] == "rm" || op.starts_with(&["network".into(), "rm".into()]))
            .collect())
    }
}

fn create_container(resources: &mut DockerResources) -> io::Result<()> {
    resources.create_container(CONTAINER_NAME, |command| {
        command.arg("postgres:17");
    })
}

fn expected_removals(container: bool) -> Vec<Vec<String>> {
    let mut calls = Vec::new();
    if container {
        calls.push(vec!["rm".into(), "--force".into(), CONTAINER.into()]);
    }
    calls.push(vec!["network".into(), "rm".into(), NETWORK.into()]);
    calls
}

#[test]
fn should_preserve_existing_network_when_name_collides_or_creation_fails() -> TestResult {
    let fake = FakeDocker::new()?;
    // Even a full existing ID on failure must not grant ownership.
    fake.reply("network", 1, FOREIGN)?;
    {
        let mut resources = fake.resources();
        let error = resources
            .create_network(NETWORK_NAME)
            .err()
            .ok_or("failed network creation accepted")?;
        assert!(!format!("{error:?} {error}").contains("private-provider-body-canary"));
    }
    assert_eq!(fake.operations()?.len(), 1);
    assert!(fake.removals()?.is_empty());
    Ok(())
}

#[test]
fn should_preserve_existing_container_after_name_collision_and_remove_only_owned_network()
-> TestResult {
    let fake = FakeDocker::new()?;
    fake.reply("container", 1, FOREIGN)?;
    {
        let mut resources = fake.resources();
        resources.create_network(NETWORK_NAME)?;
        assert!(create_container(&mut resources).is_err());
    }
    assert_eq!(fake.operations()?.len(), 3);
    assert_eq!(fake.removals()?, expected_removals(false));
    Ok(())
}

#[test]
fn should_never_clean_up_by_name_when_creation_returns_malformed_or_multiple_ids() -> TestResult {
    for output in [
        String::new(),
        "short".into(),
        "--all".into(),
        "g".repeat(64),
        "a".repeat(63),
        "A".repeat(64),
        "a".repeat(65),
        format!("{NETWORK}\n{FOREIGN}"),
    ] {
        for network_exists in [false, true] {
            let fake = FakeDocker::new()?;
            fake.reply(
                if network_exists {
                    "container"
                } else {
                    "network"
                },
                0,
                &output,
            )?;
            {
                let mut resources = fake.resources();
                if network_exists {
                    resources.create_network(NETWORK_NAME)?;
                    assert!(create_container(&mut resources).is_err());
                } else {
                    assert!(resources.create_network(NETWORK_NAME).is_err());
                }
            }
            if network_exists {
                assert_eq!(fake.removals()?, expected_removals(false));
            } else {
                assert!(fake.removals()?.is_empty());
            }
        }
    }
    Ok(())
}

#[test]
fn should_clean_up_exact_owned_ids_when_container_start_fails() -> TestResult {
    let fake = FakeDocker::new()?;
    fake.reply("start", 1, FOREIGN)?;
    {
        let mut resources = fake.resources();
        resources.create_network(NETWORK_NAME)?;
        create_container(&mut resources)?;
        assert!(resources.start_container().is_err());
    }
    let calls = fake.operations()?;
    assert_eq!(calls.len(), 5);
    assert_eq!(calls[2], vec!["start", CONTAINER]);
    assert_eq!(fake.removals()?, expected_removals(true));
    Ok(())
}

#[test]
fn should_remove_only_owned_ids_once_after_success_without_volume_cleanup() -> TestResult {
    let fake = FakeDocker::new()?;
    {
        let mut resources = fake.resources();
        resources.create_network(NETWORK_NAME)?;
        create_container(&mut resources)?;
        resources.start_container()?;
        resources.cleanup()?;
        resources.cleanup()?;
    }
    let calls = fake.operations()?;
    assert_eq!(calls.len(), 5);
    assert_eq!(
        calls[1],
        vec![
            "create",
            "--pull=never",
            "--name",
            CONTAINER_NAME,
            "--network",
            NETWORK,
            "postgres:17"
        ]
    );
    assert_eq!(fake.removals()?, expected_removals(true));
    assert!(
        calls
            .iter()
            .flatten()
            .all(|arg| !matches!(arg.as_str(), "--volumes" | "-v" | "prune"))
    );
    Ok(())
}

#[test]
fn should_report_cleanup_failure_without_targeting_names_or_unknown_resources() -> TestResult {
    let fake = FakeDocker::new()?;
    fake.reply("remove-container", 1, FOREIGN)?;
    {
        let mut resources = fake.resources();
        resources.create_network(NETWORK_NAME)?;
        create_container(&mut resources)?;
        assert!(resources.cleanup().is_err());
    }
    assert_eq!(fake.operations()?.len(), 4);
    assert_eq!(fake.removals()?, expected_removals(true));
    Ok(())
}

#[test]
fn should_not_replace_already_owned_resource_ids() -> TestResult {
    let fake = FakeDocker::new()?;
    {
        let mut resources = fake.resources();
        resources.create_network(NETWORK_NAME)?;
        create_container(&mut resources)?;
        assert!(resources.create_network("another-name").is_err());
        assert!(create_container(&mut resources).is_err());
    }
    assert_eq!(fake.operations()?.len(), 4);
    assert_eq!(fake.removals()?, expected_removals(true));
    Ok(())
}

#[test]
#[cfg(target_os = "linux")]
fn should_require_a_local_unix_socket_node() -> TestResult {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixListener;
    let directory = TestDirectory::new()?;
    assert!(require_unix_socket(&directory.0.join("missing")).is_err());
    assert!(require_unix_socket(&directory.0).is_err());
    let regular = directory.file("regular", b"not a socket", 0o600)?;
    assert!(require_unix_socket(&regular).is_err());
    // Keep the socket inside the owned directory without exceeding sockaddr_un's limit.
    let directory_fd = fs::File::open(&directory.0)?;
    let path = PathBuf::from(format!("/proc/self/fd/{}/socket", directory_fd.as_raw_fd()));
    let _listener = UnixListener::bind(&path)?;
    require_unix_socket(&directory.0.join("socket"))?;
    Ok(())
}

#[test]
fn should_ignore_remote_docker_environment_in_isolated_fake_command() -> TestResult {
    let output = run(Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "tls_tests::docker_fixture::tests::should_check_docker_environment_in_child",
            "--ignored",
        ])
        .env("DOCKER_HOST", "tcp://remote.invalid:2376")
        .env("DOCKER_CONTEXT", "remote-context")
        .env("DOCKER_CONFIG", "/untrusted/docker-config")
        .env("DOCKER_TLS_VERIFY", "1")
        .env("DOCKER_CERT_PATH", "/untrusted/cert-path")
        .env("DOCKER_API_VERSION", "1.0"))?;
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    Ok(())
}

#[test]
#[ignore = "subprocess helper; parent test injects hostile Docker environment without global mutation"]
fn should_check_docker_environment_in_child() -> TestResult {
    assert!(std::env::var_os("DOCKER_HOST").is_some());
    assert!(std::env::var_os("DOCKER_CONTEXT").is_some());
    let fake = FakeDocker::new()?;
    {
        let mut resources = fake.resources();
        resources.create_network(NETWORK_NAME)?;
        create_container(&mut resources)?;
        resources.start_container()?;
    }
    assert!(fs::read_to_string(fake.0.0.join("environment"))?.is_empty());
    assert_eq!(fake.operations()?.len(), 5);
    assert_eq!(fake.removals()?, expected_removals(true));
    Ok(())
}

//! Private local-Docker ownership; names and failed create output confer no authority.
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const LOCAL_SOCKET: &str = "/var/run/docker.sock";
const LOCAL_ENDPOINT: &str = "unix:///var/run/docker.sock";

struct ContainerId(String);

impl ContainerId {
    fn created(output: Output) -> io::Result<Self> {
        if !output.status.success() {
            return Err(io::Error::other(
                "Postgres container creation failed or timed out; no ownership acquired (output suppressed)",
            ));
        }
        let id = std::str::from_utf8(&output.stdout)
            .map_err(|_| io::Error::other("invalid created container ID; no cleanup authority"))?
            .trim_ascii();
        if id.len() != 64
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(io::Error::other(
                "invalid or missing created container ID; no cleanup by name",
            ));
        }
        Ok(Self(id.to_owned()))
    }
}

fn require_local_socket(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket()) {
            return Ok(());
        }
    }
    Err(io::Error::other(
        "Postgres fixture requires the local Docker Unix socket",
    ))
}

fn create_config_directory() -> io::Result<PathBuf> {
    // Exclusive creation is the only authority to remove this directory later.
    let path = Path::new("/tmp").join(format!("aura-test-postgres-{}", uuid::Uuid::new_v4()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(&path)
        .map_err(|_| io::Error::other("could not create private Docker fixture config"))?;
    Ok(path)
}

pub(super) struct DockerFixture {
    program: PathBuf,
    config_dir: PathBuf,
    container: Option<ContainerId>,
}

impl DockerFixture {
    pub(super) fn local() -> io::Result<Self> {
        require_local_socket(Path::new(LOCAL_SOCKET))?;
        Ok(Self {
            program: "/usr/bin/docker".into(),
            config_dir: create_config_directory()?,
            container: None,
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new("/usr/bin/timeout");
        command
            .args(["--signal=TERM", "--kill-after=2s", "20s"])
            .arg(&self.program)
            .args(["--host", LOCAL_ENDPOINT, "--config"])
            .arg(&self.config_dir)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null());
        command
    }

    pub(super) fn create(&mut self, name: &str, image: &str) -> io::Result<()> {
        if self.container.is_some() {
            return Err(io::Error::other(
                "Postgres fixture already owns a container",
            ));
        }
        // The override is an intentional local image, never CLI options or a pull request.
        if image.is_empty()
            || image.starts_with('-')
            || image
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte == 0)
        {
            return Err(io::Error::other(
                "invalid local Postgres fixture image reference",
            ));
        }
        let output = self
            .command()
            .args([
                "create",
                "--pull=never",
                "--name",
                name,
                "--publish",
                // Loopback-only publication breaks Sequin's host-gateway connection.
                "0.0.0.0::5432",
                "--tmpfs",
                "/var/lib/postgresql/data:rw,nosuid",
                "--env",
                "POSTGRES_USER=postgres",
                "--env",
                "POSTGRES_PASSWORD=postgres",
                "--env",
                "POSTGRES_DB=postgres",
                image,
                "postgres",
                "-c",
                "fsync=off",
                "-c",
                "wal_level=logical",
                "-c",
                "shared_preload_libraries=pg_ttl_index",
            ])
            .output()
            .map_err(|_| io::Error::other("Postgres Docker create command unavailable"))?;
        self.container = Some(ContainerId::created(output)?);
        Ok(())
    }

    fn id(&self) -> io::Result<&str> {
        self.container
            .as_ref()
            .map(|id| id.0.as_str())
            .ok_or_else(|| io::Error::other("Postgres fixture has no owned container"))
    }

    pub(super) fn start(&self) -> io::Result<u16> {
        checked(self.command().args(["start", self.id()?]))?;
        let output = checked(self.command().args([
            "inspect",
            "--format",
            "{{(index (index .NetworkSettings.Ports \"5432/tcp\") 0).HostPort}}",
            self.id()?,
        ]))?;
        let port = std::str::from_utf8(&output.stdout)
            .map_err(|_| io::Error::other("invalid owned Postgres container host port"))?
            .trim_ascii()
            .parse::<u16>()
            .map_err(|_| io::Error::other("invalid owned Postgres container host port"))?;
        if port == 0 {
            return Err(io::Error::other(
                "invalid owned Postgres container host port",
            ));
        }
        Ok(port)
    }

    fn exists(&self, id: &str) -> io::Result<bool> {
        let output = checked(self.command().args([
            "container",
            "ls",
            "--all",
            "--no-trunc",
            "--quiet",
            "--filter",
            &format!("id={id}"),
        ]))?;
        if output.stdout.trim_ascii().is_empty() {
            return Ok(false);
        }
        if output.stdout.trim_ascii() == id.as_bytes() {
            return Ok(true);
        }
        Err(io::Error::other(
            "unexpected owned-container lookup response",
        ))
    }

    pub(super) fn cleanup(&mut self) -> io::Result<()> {
        let Some(id) = &self.container else {
            return Ok(());
        };
        // Keep authority on failure so exit hooks can retry. Never infer volume ownership.
        let result = (|| {
            if self.exists(&id.0)? {
                checked(self.command().args(["rm", "--force", &id.0]))?;
                if self.exists(&id.0)? {
                    return Err(io::Error::other(
                        "owned container still exists after removal",
                    ));
                }
            }
            Ok(())
        })();
        result.map_err(|_: io::Error| {
            io::Error::other(format!(
                "owned Postgres container {} cleanup failed on local Unix Docker (output suppressed)",
                id.0
            ))
        })?;
        self.container = None;
        Ok(())
    }
}

fn checked(command: &mut Command) -> io::Result<Output> {
    let output = command
        .output()
        .map_err(|_| io::Error::other("local Postgres Docker command unavailable"))?;
    if !output.status.success() {
        return Err(io::Error::other(
            "local Postgres Docker command failed or timed out (output suppressed)",
        ));
    }
    Ok(output)
}

impl Drop for DockerFixture {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("{error}");
        }
        if fs::remove_dir_all(&self.config_dir).is_err() {
            eprintln!("owned Postgres Docker config cleanup failed (details suppressed)");
        }
    }
}

#[cfg(all(test, unix))]
#[path = "postgres_fixture_tests.rs"]
mod tests;

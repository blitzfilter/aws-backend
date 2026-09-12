//! Test-only Docker ownership. Names never confer cleanup authority.
use crate::test_support::run;
use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const LOCAL_SOCKET: &str = "/var/run/docker.sock";
const LOCAL_ENDPOINT: &str = "unix:///var/run/docker.sock";

struct DockerId(String);

impl DockerId {
    fn created(output: Output) -> io::Result<Self> {
        if !output.status.success() {
            return Err(io::Error::other(
                "Docker creation failed; no ownership acquired",
            ));
        }
        let id = std::str::from_utf8(&output.stdout)
            .map_err(|_| io::Error::other("Docker creation returned an invalid ID"))?
            .trim_ascii();
        if id.len() != 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(io::Error::other(
                "Docker creation returned an invalid ID; no cleanup by name",
            ));
        }
        Ok(Self(id.to_owned()))
    }
}

fn require_unix_socket(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket()) {
            return Ok(());
        }
    }
    Err(io::Error::other(
        "TLS fixture requires the local Docker Unix socket",
    ))
}

pub(super) struct DockerResources {
    program: PathBuf,
    config_dir: PathBuf,
    network: Option<DockerId>,
    container: Option<DockerId>,
}

impl DockerResources {
    pub(super) fn local(directory: &Path) -> io::Result<Self> {
        require_unix_socket(Path::new(LOCAL_SOCKET))?;
        let config_dir = directory.join("docker-config");
        fs::create_dir(&config_dir)?;
        Ok(Self {
            program: "/usr/bin/docker".into(),
            config_dir,
            network: None,
            container: None,
        })
    }

    pub(super) fn command(&self) -> Command {
        let mut command = Command::new("/usr/bin/timeout");
        command
            .arg("20s")
            .arg(&self.program)
            .args(["--host", LOCAL_ENDPOINT, "--config"])
            .arg(&self.config_dir)
            .env_clear()
            .env("PATH", "/usr/bin:/bin");
        command
    }

    pub(super) fn create_network(&mut self, name: &str) -> io::Result<()> {
        if self.network.is_some() {
            return Err(io::Error::other("fixture already owns a Docker network"));
        }
        let output = run(self
            .command()
            .args(["network", "create", "--driver", "bridge", name]))?;
        self.network = Some(DockerId::created(output)?);
        Ok(())
    }

    pub(super) fn create_container(
        &mut self,
        name: &str,
        configure: impl FnOnce(&mut Command),
    ) -> io::Result<()> {
        if self.container.is_some() {
            return Err(io::Error::other("fixture already owns a Docker container"));
        }
        let network = self
            .network
            .as_ref()
            .ok_or_else(|| io::Error::other("fixture has no owned Docker network"))?;
        let mut command = self.command();
        command.args([
            "create",
            "--pull=never",
            "--name",
            name,
            "--network",
            &network.0,
        ]);
        configure(&mut command);
        self.container = Some(DockerId::created(run(&mut command)?)?);
        Ok(())
    }

    pub(super) fn container_id(&self) -> io::Result<&str> {
        self.container
            .as_ref()
            .map(|id| id.0.as_str())
            .ok_or_else(|| io::Error::other("fixture has no owned Docker container"))
    }

    pub(super) fn start_container(&self) -> io::Result<()> {
        run(self.command().args(["start", self.container_id()?]))?;
        Ok(())
    }

    pub(super) fn cleanup(&mut self) -> io::Result<()> {
        let mut failed = false;
        if let Some(id) = self.container.take() {
            // No --volumes: never infer ownership of image/existing attached volumes.
            failed |= run(self.command().args(["rm", "--force", &id.0])).is_err();
        }
        if let Some(id) = self.network.take() {
            failed |= run(self.command().args(["network", "rm", &id.0])).is_err();
        }
        if failed {
            return Err(io::Error::other(
                "owned Docker resource cleanup failed (details suppressed)",
            ));
        }
        Ok(())
    }
}

impl Drop for DockerResources {
    fn drop(&mut self) {
        if self.cleanup().is_err() {
            eprintln!("owned Docker resource cleanup failed (details suppressed)");
        }
    }
}

#[cfg(all(test, unix))]
#[path = "docker_fixture_tests.rs"]
mod tests;

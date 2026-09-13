use std::{
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) fn assert_redacted_chain(error: &(dyn std::error::Error + 'static), canaries: &[&str]) {
    let mut current = Some(error);
    let mut count = 0;
    while let Some(member) = current {
        let formatted = format!("{member} {member:?} {member:#?}");
        assert!(
            canaries.iter().all(|canary| !formatted.contains(canary)),
            "error chain leaked a canary"
        );
        assert!(!member.is::<sqlx::Error>(), "raw SQLx cause escaped");
        count += 1;
        assert!(count <= 2, "unexpected connection error source chain");
        current = member.source();
    }
    assert_eq!(count, 2);
}

pub(crate) struct TestDirectory(pub(crate) PathBuf);

impl TestDirectory {
    pub(crate) fn new() -> io::Result<Self> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| io::Error::other("fixture clock failed"))?
            .as_nanos();
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "postgres-policy-{}-{stamp}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // A collision must never confer ownership of an existing directory.
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }

    pub(crate) fn file(&self, name: &str, content: &[u8], mode: u32) -> io::Result<PathBuf> {
        let path = self.0.join(name);
        fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        }
        Ok(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        if fs::remove_dir_all(&self.0).is_err() {
            eprintln!("PostgreSQL policy fixture directory cleanup failed (path suppressed)");
        }
    }
}

pub(crate) fn run(command: &mut Command) -> io::Result<Output> {
    // Only fixed operation labels may enter diagnostics, never arguments or provider output.
    let step = match command.get_args().nth(2).and_then(|arg| arg.to_str()) {
        Some("req") => "certificate request",
        Some("x509") => "certificate signing",
        Some("rand") => "password generation",
        Some("network") => "Docker network creation",
        Some("run") => "Docker container creation",
        Some("inspect") => "Docker port lookup",
        _ => "test subprocess",
    };
    let output = command
        .output()
        .map_err(|_| io::Error::other(format!("{step} unavailable (details suppressed)")))?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{step} failed with {} (output suppressed)",
            output.status
        )));
    }
    Ok(output)
}

pub(crate) fn openssl() -> Command {
    let mut command = Command::new("timeout");
    command.args(["15s", "openssl"]);
    command
}

pub(crate) fn generate_ca(directory: &TestDirectory, name: &str) -> io::Result<Vec<u8>> {
    run(openssl()
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-noenc",
            "-days",
            "1",
            "-subj",
            "/CN=Postgres-policy-test-CA",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
            "-keyout",
        ])
        .arg(directory.0.join(format!("{name}.key")))
        .arg("-out")
        .arg(directory.0.join(format!("{name}.crt"))))?;
    fs::read(directory.0.join(format!("{name}.crt")))
}

pub(crate) fn root_pem() -> io::Result<Vec<u8>> {
    static ROOT: std::sync::LazyLock<io::Result<Vec<u8>>> = std::sync::LazyLock::new(|| {
        let directory = TestDirectory::new()?;
        generate_ca(&directory, "ca")
    });
    match &*ROOT {
        Ok(pem) => Ok(pem.clone()),
        Err(_) => Err(io::Error::other("ephemeral test CA generation failed")),
    }
}

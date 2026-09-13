use super::*;
use crate::support::Directory;
use std::{fs, path::Path};

const ENTRY: &str = "process::tests::should_run_isolated_process_helper";
const WAIT: Duration = Duration::from_secs(3);

fn helper(mode: &str, directory: &Path) -> TestResult<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            ENTRY,
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", directory)
        .env("CRAWLER_PROCESS_REGRESSION", mode)
        .env("CRAWLER_PROCESS_DIRECTORY", directory)
        .current_dir(directory);
    Ok(command)
}

fn isolated(mode: &str) -> TestResult {
    let directory = Directory::new()?;
    let result = (|| {
        let output = run_process(
            &mut helper(mode, &directory.0)?,
            Duration::from_secs(15),
            CleanupMode::Kill,
        )?;
        if !output.status.success() {
            return Err(TestError::failure("ISOLATED_REGRESSION_FAILED"));
        }
        Ok(())
    })();
    directory.close()?;
    result
}

#[test]
fn should_use_fixture_sigterm_and_join_started_readers_after_partial_setup_failure() -> TestResult {
    isolated("partial")
}

#[test]
fn should_bound_and_join_readers_when_descendant_holds_output_after_owner_exit() -> TestResult {
    isolated("descendant")
}

fn wait_file(path: &Path) -> TestResult {
    let deadline = Instant::now() + WAIT;
    while !path.try_exists()? {
        if Instant::now() >= deadline {
            return Err(TestError::failure("REGRESSION_MARKER_TIMEOUT"));
        }
        thread::sleep(POLL);
    }
    Ok(())
}

fn partial_readers(directory: &Path) -> TestResult {
    for (index, unwind) in [(0, false), (1, false), (1, true)] {
        let case = directory.join(format!("partial-{index}-{unwind}"));
        fs::create_dir(&case)?;
        let mut command = Command::new("/usr/bin/sh");
        command.args(["-c",
            "trap ': > terminated; exit 0' TERM; printf 'unrecognized-private-provider-body'; : > ready; while :; do /usr/bin/sleep 0.01; done"
        ]).env_clear().env("PATH", "/usr/bin:/bin").current_dir(&case);
        let mut state = None;
        let started = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Process::spawn_with_reader_hook(
                &mut command,
                WAIT,
                CleanupMode::FixtureHooks,
                |next, process| {
                    if next != index {
                        return Ok(());
                    }
                    wait_file(&case.join("ready"))?;
                    state = Some(process.reader_state.clone());
                    if unwind {
                        // Exercise Drop without invoking a panic hook that prints the payload.
                        std::panic::resume_unwind(Box::new("unrecognized-private-provider-body"));
                    }
                    Err(TestError::caused(
                        "READER_START",
                        io::Error::other("unrecognized-private-provider-body"),
                    ))
                },
            )
        }));
        match result {
            Ok(Err(error)) if !unwind => {
                assert!(error.kind() == "READER_START");
                assert!(
                    !format!("{error} {error:?} {error:#?}")
                        .contains("unrecognized-private-provider-body")
                );
            }
            Err(_) if unwind => {}
            _ => return Err(TestError::failure("PARTIAL_READER_FAILURE_NOT_EXERCISED")),
        }
        let state = state.ok_or("reader failure hook not reached")?;
        assert!(
            case.join("terminated").try_exists()?,
            "fixture cleanup skipped SIGTERM hook"
        );
        assert!(
            state.reaped.load(Ordering::Acquire),
            "direct child was not reaped"
        );
        assert!(state.started.load(Ordering::Acquire) == index);
        assert!(state.completed.load(Ordering::Acquire) == index);
        assert!(
            state.joined.load(Ordering::Acquire) == index,
            "started output reader was not joined"
        );
        assert!(
            started.elapsed() < WAIT,
            "partial-setup cleanup exceeded its regression budget"
        );
    }
    Ok(())
}

struct AdoptedChild {
    pid: libc::pid_t,
    reaped: bool,
}
impl AdoptedChild {
    fn poll(&mut self, budget: Duration) -> TestResult {
        let deadline = Instant::now() + budget;
        loop {
            let mut status = 0;
            // SAFETY: this isolated subreaper owns the adopted child from the private
            // helper's successful spawn record; waitpid targets only that positive PID.
            let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if result == self.pid {
                self.reaped = true;
                return Ok(());
            }
            if result < 0 {
                return Err(TestError::caused(
                    "DESCENDANT_REAP",
                    io::Error::last_os_error(),
                ));
            }
            if Instant::now() >= deadline {
                return Err(TestError::failure("DESCENDANT_REAP_TIMEOUT"));
            }
            thread::sleep(POLL);
        }
    }

    fn close(&mut self) -> TestResult {
        if self.reaped {
            return Ok(());
        }
        // SAFETY: the recorded child is still owned and unreaped, so its PID cannot
        // be reused. No process-group, host-process scan, or container operation.
        let signal = unsafe { libc::kill(self.pid, libc::SIGKILL) };
        let error = if signal == 0 {
            None
        } else {
            Some(io::Error::last_os_error())
        };
        self.poll(REAP_GRACE)?;
        if let Some(error) = error {
            return Err(TestError::caused("DESCENDANT_KILL", error));
        }
        Ok(())
    }
}
impl Drop for AdoptedChild {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("owned regression cleanup failed: {}", error.kind());
            std::process::exit(1);
        }
    }
}

fn descendant(directory: &Path) -> TestResult {
    // SAFETY: Linux changes only this isolated regression subprocess. Its only children
    // come from the helper below; adoption lets us explicitly reap the orphaned holder.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } != 0 {
        return Err(TestError::caused(
            "REGRESSION_SUBREAPER",
            io::Error::last_os_error(),
        ));
    }
    let mut owner = Process::spawn(&mut helper("owner", directory)?, WAIT, CleanupMode::Kill)?;
    let status = owner.poll(WAIT)?;
    let pid: libc::pid_t = fs::read_to_string(directory.join("holder-pid"))?.parse()?;
    if pid <= 1 {
        return Err(TestError::failure("INVALID_OWNED_DESCENDANT_RECORD"));
    }
    let mut holder = AdoptedChild { pid, reaped: false };
    let result: TestResult = (|| {
        wait_file(&directory.join("holder-ready"))?;
        assert!(status.success());
        let started = Instant::now();
        let drained = owner.cleanup();
        assert!(
            matches!(drained, Err(error) if error.kind() == "PIPE_EOF_TIMEOUT"),
            "descendant-held stdout/stderr was treated as complete output"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(owner.readers.is_empty(), "output ownership was detached");
        assert!(owner.reader_state.completed.load(Ordering::Acquire) == 2);
        assert!(owner.reader_state.joined.load(Ordering::Acquire) == 2);
        assert!(
            !directory.join("holder-done").try_exists()?,
            "holder exited before reader deadline"
        );
        Ok(())
    })();
    // Release and reap before assertions escape, including on a reader-cleanup failure.
    let release = fs::write(directory.join("release-holder"), b"release");
    let reaped = holder.poll(WAIT);
    if reaped.is_err() {
        holder.close()?;
    }
    release?;
    reaped?;
    result
}

#[test]
#[ignore = "internal no-Docker subprocess entry; run the ordinary process regression tests"]
fn should_run_isolated_process_helper() -> TestResult {
    let mode = std::env::var("CRAWLER_PROCESS_REGRESSION")?;
    let directory =
        std::env::var_os("CRAWLER_PROCESS_DIRECTORY").ok_or("missing regression directory")?;
    let directory = Path::new(&directory);
    match mode.as_str() {
        "partial" => partial_readers(directory),
        "descendant" => descendant(directory),
        "owner" => {
            let mut holder = helper("holder", directory)?
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?;
            let record = fs::write(directory.join("holder-pid"), holder.id().to_string());
            if let Err(error) = record {
                holder.kill()?;
                let deadline = Instant::now() + REAP_GRACE;
                while holder.try_wait()?.is_none() {
                    if Instant::now() >= deadline {
                        return Err(TestError::failure("HOLDER_REAP_TIMEOUT"));
                    }
                    thread::sleep(POLL);
                }
                return Err(error.into());
            }
            // Deliberate owner exit: its descendant retains inherited stdout and stderr.
            // Reap/cleanup authority transfers via the record to our isolated subreaper.
            Ok(())
        }
        "holder" => {
            fs::write(directory.join("holder-ready"), b"ready")?;
            let deadline = Instant::now() + Duration::from_secs(8);
            while !directory.join("release-holder").try_exists()? {
                if Instant::now() >= deadline {
                    return Err(TestError::failure("HOLDER_RELEASE_TIMEOUT"));
                }
                thread::sleep(POLL);
            }
            fs::write(directory.join("holder-done"), b"done")?;
            Ok(())
        }
        _ => Err(TestError::failure("UNKNOWN_REGRESSION_MODE")),
    }
}

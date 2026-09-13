use std::{io, thread, time::Duration};

/// One-shot CLI guard, deliberately alive until process exit. No logging on expiry.
/// Never use this for an unbounded daemon; its lifecycle needs a separately armed drain guard.
pub(super) fn arm(timeout: Duration) -> io::Result<()> {
    thread::Builder::new()
        .name("crawler-preflight-deadline".into())
        .spawn(move || {
            thread::sleep(timeout);
            super::shutdown::terminal_exit(1);
        })?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::{
            fd::{AsFd, OwnedFd},
            unix::net::UnixStream,
        },
        process::{Command, Stdio},
        time::Instant,
    };

    #[test]
    fn should_exit_without_logging_when_error_output_is_blocked() -> io::Result<()> {
        let (_reader, mut writer) = UnixStream::pair()?;
        writer.set_nonblocking(true)?;
        loop {
            match writer.write(&[b'x'; 4096]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        writer.set_nonblocking(false)?;
        let mut child = Command::new(std::env::current_exe()?)
            .env_clear()
            .args([
                "--exact",
                "watchdog::tests::blocked_output_child",
                "--ignored",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(OwnedFd::from(writer)))
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    assert_eq!(status.code(), Some(1));
                    return Ok(());
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                result => {
                    let killed = child.kill();
                    let reaped = child.wait();
                    killed?;
                    reaped?;
                    return Err(io::Error::other(format!(
                        "watchdog child did not exit: {result:?}"
                    )));
                }
            }
        }
    }

    #[rstest::rstest]
    #[case("terminal", 0)]
    #[case("daemon_flush", 1)]
    #[case("rust_cleanup", 1)]
    fn should_fence_buffered_stdout_without_entering_shared_cleanup(
        #[case] mode: &str,
        #[case] expected: i32,
    ) -> io::Result<()> {
        let (_reader, writer) = UnixStream::pair()?;
        let mut child = Command::new(std::env::current_exe()?)
            .env_clear()
            .env("CRAWLER_EXIT_TEST", mode)
            .args([
                "--exact",
                "watchdog::tests::buffered_stdout_child",
                "--ignored",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::from(OwnedFd::from(writer)))
            .stderr(Stdio::null())
            .spawn()?;
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    assert_eq!(status.code(), Some(expected));
                    return Ok(());
                }
                Ok(None) if Instant::now() < end => thread::sleep(Duration::from_millis(5)),
                result => {
                    let killed = child.kill();
                    let reaped = child.wait();
                    killed?;
                    reaped?;
                    return Err(io::Error::other(format!(
                        "buffered exit was not fenced: {result:?}"
                    )));
                }
            }
        }
    }

    #[test]
    #[ignore = "subprocess helper; child fills only its own stdout socket"]
    fn buffered_stdout_child() -> io::Result<()> {
        use crate::shutdown::{ProcessShutdown, terminal_exit};
        let mode = std::env::var("CRAWLER_EXIT_TEST").map_err(io::Error::other)?;
        let shutdown = ProcessShutdown::install(crate::config::LifecycleConfig::default())?;
        shutdown.lifecycle.stop(false);
        shutdown.lifecycle.begin_cleanup();
        shutdown.lifecycle.begin_teardown();
        let observer = shutdown.lifecycle.clone();
        thread::spawn(move || {
            loop {
                if observer.state() == crate::lifecycle::CrawlerState::Stopped {
                    // A stop state followed by a stalled flush must never masquerade as exit.
                    thread::sleep(Duration::from_millis(50));
                    terminal_exit(2);
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        if mode != "daemon_flush" {
            arm(Duration::from_millis(100))?;
        }
        io::stdout().write_all(b"buffered-without-newline")?;
        let mut direct = UnixStream::from(io::stdout().as_fd().try_clone_to_owned()?);
        direct.set_nonblocking(true)?;
        loop {
            match direct.write(&[b'x'; 4096]) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        direct.set_nonblocking(false)?;
        match mode.as_str() {
            "terminal" => terminal_exit(0),
            "daemon_flush" => shutdown.exit(true),
            // Deliberately reproduce the old Rust cleanup Once stall. Only this fixture
            // calls std::process::exit; the real one-shot watchdog must bypass its lock.
            "rust_cleanup" => std::process::exit(0),
            _ => Err(io::Error::other("unknown terminal fixture mode")),
        }
    }

    #[test]
    #[ignore = "subprocess helper; parent supplies blocked stderr"]
    fn blocked_output_child() -> io::Result<()> {
        arm(Duration::from_millis(100))?;
        // More than the filled socket's residual capacity; expiry must not log to this stream.
        io::stderr().write_all(&[b'x'; 64 * 1024])?;
        Err(io::Error::other("blocked output unexpectedly completed"))
    }
}

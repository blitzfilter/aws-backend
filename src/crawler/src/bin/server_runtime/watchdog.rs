use std::{io, thread, time::Duration};

/// One-shot CLI guard, deliberately alive until process exit. No logging on expiry.
/// Never use this for an unbounded daemon; its lifecycle needs a separately armed drain guard.
pub(super) fn arm(timeout: Duration) -> io::Result<()> {
    thread::Builder::new()
        .name("crawler-preflight-deadline".into())
        .spawn(move || {
            thread::sleep(timeout);
            std::process::exit(1);
        })?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::{fd::OwnedFd, unix::net::UnixStream},
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

    #[test]
    #[ignore = "subprocess helper; parent supplies blocked stderr"]
    fn blocked_output_child() -> io::Result<()> {
        arm(Duration::from_millis(100))?;
        // More than the filled socket's residual capacity; expiry must not log to this stream.
        io::stderr().write_all(&[b'x'; 64 * 1024])?;
        Err(io::Error::other("blocked output unexpectedly completed"))
    }
}

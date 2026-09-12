use super::error::{TestError, TestResult};
use std::{
    io::{self, Read},
    os::fd::AsRawFd,
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_millis(5);
const PIPE_EOF_GRACE: Duration = Duration::from_millis(250);
const READER_JOIN_GRACE: Duration = Duration::from_secs(2);
const REAP_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
pub(super) enum CleanupMode {
    Kill,
    FixtureHooks,
}
impl CleanupMode {
    fn signal_grace(self) -> Duration {
        match self {
            Self::Kill => Duration::ZERO,
            Self::FixtureHooks => Duration::from_secs(70),
        }
    }
}

#[derive(Default)]
struct ReaderState {
    started: AtomicUsize,
    completed: AtomicUsize,
    joined: AtomicUsize,
    reaped: AtomicBool,
}
struct ReaderCompletion(Arc<ReaderState>);
impl Drop for ReaderCompletion {
    fn drop(&mut self) {
        self.0.completed.fetch_add(1, Ordering::Release);
    }
}

type Reader = JoinHandle<TestResult<Vec<u8>>>;

struct Process {
    child: Child,
    mode: CleanupMode,
    readers: Vec<Reader>,
    cancel_readers: Arc<AtomicBool>,
    reader_state: Arc<ReaderState>,
    reader_deadline: Instant,
    status: Option<ExitStatus>,
    cleanup_attempted: bool,
}

impl Process {
    fn spawn(command: &mut Command, budget: Duration, mode: CleanupMode) -> TestResult<Self> {
        Self::spawn_with_reader_hook(command, budget, mode, |_, _| Ok(()))
    }

    fn spawn_with_reader_hook(
        command: &mut Command,
        budget: Duration,
        mode: CleanupMode,
        mut before_reader: impl FnMut(usize, &Self) -> TestResult,
    ) -> TestResult<Self> {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| TestError::caused("CHILD_START", error))?;
        // Cleanup policy is owned from acquisition, before any fallible pipe/thread setup.
        let mut process = Self {
            child,
            mode,
            readers: Vec::new(),
            cancel_readers: Arc::new(AtomicBool::new(false)),
            reader_state: Arc::new(ReaderState::default()),
            reader_deadline: Instant::now()
                + budget
                + mode.signal_grace()
                + REAP_GRACE
                + PIPE_EOF_GRACE
                + READER_JOIN_GRACE,
            status: None,
            cleanup_attempted: false,
        };
        let setup: TestResult = (|| {
            let stdout = process.child.stdout.take().ok_or("missing child stdout")?;
            let stderr = process.child.stderr.take().ok_or("missing child stderr")?;
            nonblocking(&stdout)?;
            nonblocking(&stderr)?;
            before_reader(0, &process)?;
            process.start_reader(stdout)?;
            before_reader(1, &process)?;
            process.start_reader(stderr)
        })();
        if let Err(error) = setup {
            let cleanup = process.cleanup();
            cleanup?;
            return Err(error);
        }
        Ok(process)
    }

    fn start_reader(&mut self, pipe: impl Read + Send + 'static) -> TestResult {
        let cancel = self.cancel_readers.clone();
        let state = self.reader_state.clone();
        let deadline = self.reader_deadline;
        let reader = thread::Builder::new()
            .spawn(move || {
                state.started.fetch_add(1, Ordering::Release);
                let _completion = ReaderCompletion(state);
                read_output(pipe, &cancel, deadline)
            })
            .map_err(|error| TestError::caused("READER_START", error))?;
        self.readers.push(reader);
        Ok(())
    }

    fn try_reap(&mut self) -> TestResult<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self
                .child
                .try_wait()
                .map_err(|error| TestError::caused("CHILD_REAP", error))?;
        }
        if self.status.is_some() {
            self.reader_state.reaped.store(true, Ordering::Release);
        }
        Ok(self.status)
    }

    fn poll(&mut self, budget: Duration) -> TestResult<ExitStatus> {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(status) = self.try_reap()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                return Err(TestError::failure("CHILD_DEADLINE"));
            }
            thread::sleep(POLL);
        }
    }

    fn stop(&mut self) -> TestResult {
        if self.try_reap()?.is_some() {
            return Ok(());
        }
        let mut signal_error = None;
        if matches!(self.mode, CleanupMode::FixtureHooks) {
            // No helper process/thread allocation during partial reader-start failure.
            // SAFETY: Child owns this positive PID and it has not been reaped. SIGTERM
            // targets that one process, never a process group, name, or container lookup.
            if unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) } != 0 {
                signal_error = Some(TestError::caused("CHILD_TERM", io::Error::last_os_error()));
            } else {
                match self.poll(self.mode.signal_grace()) {
                    Ok(_) => return Ok(()),
                    Err(error) if error.kind() == "CHILD_DEADLINE" => {}
                    Err(error) => signal_error = Some(error),
                }
            }
        }
        let killed = self
            .child
            .kill()
            .map_err(|error| TestError::caused("CHILD_KILL", error));
        // Even failed signalling still requires a bounded reap attempt. Never use wait().
        let reaped = self
            .poll(REAP_GRACE)
            .map_err(|error| error.context("CHILD_REAP_TIMEOUT"));
        reaped?;
        killed?;
        if let Some(error) = signal_error {
            return Err(error);
        }
        Ok(())
    }

    fn join_readers(&mut self) -> TestResult<Vec<Vec<u8>>> {
        let eof_deadline = Instant::now() + PIPE_EOF_GRACE;
        while self.readers.iter().any(|reader| !reader.is_finished())
            && Instant::now() < eof_deadline
        {
            thread::sleep(POLL);
        }
        // A descendant can keep either inherited writer open after the direct child exits.
        // Nonblocking readers must close their owned read ends rather than wait for that EOF.
        self.cancel_readers.store(true, Ordering::Release);
        let join_deadline = Instant::now() + READER_JOIN_GRACE;
        while self.readers.iter().any(|reader| !reader.is_finished())
            && Instant::now() < join_deadline
        {
            thread::sleep(POLL);
        }
        let mut outputs = Vec::new();
        let mut failures = Vec::new();
        let mut unfinished = Vec::new();
        // Join every completed owner, even when an earlier reader returned an error/panic.
        for reader in self.readers.drain(..) {
            if !reader.is_finished() {
                unfinished.push(reader);
                continue;
            }
            match reader.join() {
                Ok(Ok(bytes)) => outputs.push(bytes),
                Ok(Err(error)) => failures.push(error),
                Err(_) => failures.push(TestError::failure("READER_PANIC")),
            }
            self.reader_state.joined.fetch_add(1, Ordering::Release);
        }
        self.readers = unfinished;
        if !self.readers.is_empty() {
            return Err(TestError::failure("READER_JOIN_TIMEOUT"));
        }
        if let Some(error) = failures.into_iter().next() {
            return Err(error);
        }
        Ok(outputs)
    }

    fn cleanup(&mut self) -> TestResult<Vec<Vec<u8>>> {
        self.cleanup_attempted = true;
        let stopped = self.stop();
        let readers = self.join_readers();
        stopped?;
        readers
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if !self.cleanup_attempted
            && let Err(error) = self.cleanup()
        {
            eprintln!("owned process cleanup failed: {}", error.kind());
        }
        if self.status.is_none() || !self.readers.is_empty() {
            // Do not detach a live reader or pretend an unconfirmed child is stopped.
            // This terminates only this test process; test-api's exit hooks retain their
            // own container authority. Its parent observes failure, never successful cleanup.
            eprintln!("owned process cleanup failed: OWNERSHIP_UNCONFIRMED");
            std::process::exit(1);
        }
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> TestResult {
    // SAFETY: the borrowed, owned pipe descriptor stays live across both fcntl calls;
    // only its existing read-end status flags are updated before spawning its reader.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(TestError::caused(
            "PIPE_NONBLOCKING",
            io::Error::last_os_error(),
        ));
    }
    Ok(())
}

fn read_output(mut pipe: impl Read, cancel: &AtomicBool, deadline: Instant) -> TestResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(TestError::failure("PIPE_EOF_TIMEOUT"));
        }
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(bytes),
            Ok(count) => {
                let retained = count.min(65536usize.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buffer[..retained]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(TestError::caused("PIPE_READ", error)),
        }
    }
}

pub(super) fn run_process(
    command: &mut Command,
    budget: Duration,
    mode: CleanupMode,
) -> TestResult<Output> {
    let mut process = Process::spawn(command, budget, mode)?;
    let status = process.poll(budget);
    let readers = process.cleanup();
    let mut readers = readers?.into_iter();
    Ok(Output {
        status: status?,
        stdout: readers.next().ok_or("missing stdout result")?,
        stderr: readers.next().ok_or("missing stderr result")?,
    })
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;

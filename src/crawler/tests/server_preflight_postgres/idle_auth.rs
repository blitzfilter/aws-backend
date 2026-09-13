//! Explicit local ADC refresh boundary. Never forwards, resolves, or opens an outbound socket.
//! google-cloud-auth 1.16 starts TokenCache refresh tasks during credential construction.
use super::error::{TestError, TestResult};
use serde_json::{Value, json};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub(super) struct AuthSpy {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    worker: Option<JoinHandle<TestResult>>,
}
impl AuthSpy {
    pub(super) fn start() -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let worker_stop = stop.clone();
        let worker_requests = requests.clone();
        let worker = thread::Builder::new().spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !worker_stop.load(Ordering::Acquire) {
                if Instant::now() >= deadline {
                    return Err(TestError::failure("IDLE_AUTH_DEADLINE"));
                }
                match listener.accept() {
                    Ok((stream, peer)) => {
                        if !peer.ip().is_loopback() {
                            return Err(TestError::failure("IDLE_AUTH_REQUEST"));
                        }
                        serve(stream, &worker_requests)?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        })?;
        Ok(Self {
            address,
            stop,
            requests,
            worker: Some(worker),
        })
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }
    pub(super) fn requests(&self) -> usize {
        self.requests.load(Ordering::Acquire)
    }

    pub(super) fn close(&mut self) -> TestResult {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = &self.worker {
            let end = Instant::now() + Duration::from_secs(2);
            while !worker.is_finished() && Instant::now() < end {
                thread::sleep(Duration::from_millis(5));
            }
            if !worker.is_finished() {
                // Never detach an unconfirmed helper. Supervised fixture hooks retain cleanup.
                eprintln!("owned idle auth cleanup failed: IDLE_AUTH_JOIN");
                std::process::exit(1);
            }
        }
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| TestError::failure("IDLE_AUTH_JOIN"))??;
        }
        Ok(())
    }
}
impl Drop for AuthSpy {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("owned idle auth cleanup failed: {}", error.kind());
        }
    }
}

fn serve(mut stream: TcpStream, requests: &AtomicUsize) -> TestResult {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut bytes = Vec::new();
    loop {
        if Instant::now() >= deadline || bytes.len() >= 4096 {
            return Err(TestError::failure("IDLE_AUTH_REQUEST"));
        }
        let mut buffer = [0_u8; 1024];
        match stream.read(&mut buffer) {
            // Negative startup cases may cancel the provider task mid-request.
            Ok(0) => return Ok(()),
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => return Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error.into()),
        }
        let Some(split) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&bytes[..split])?;
        if !headers.starts_with("POST /token HTTP/1.1\r\n") {
            return Err(TestError::failure("IDLE_AUTH_REQUEST"));
        }
        let headers = headers.to_ascii_lowercase();
        let length: usize = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length: "))
            .ok_or_else(|| TestError::failure("IDLE_AUTH_REQUEST"))?
            .trim()
            .parse()?;
        if length > 2048 {
            return Err(TestError::failure("IDLE_AUTH_REQUEST"));
        }
        if bytes.len() < split + 4 + length {
            continue;
        }
        let body: Value = serde_json::from_slice(&bytes[split + 4..])?;
        if body
            != json!({
                "grant_type": "refresh_token",
                "client_id": "idle-client-canary",
                "client_secret": "idle-secret-canary",
                "refresh_token": "idle-refresh-canary",
                "scopes": "https://www.googleapis.com/auth/cloud-platform",
            })
        {
            return Err(TestError::failure("IDLE_AUTH_REQUEST"));
        }
        requests.fetch_add(1, Ordering::Release);
        let body = json!({"access_token": "idle-access-canary", "token_type": "Bearer", "expires_in": 3600}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        match stream.write_all(response.as_bytes()) {
            Ok(()) => return Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect(spy: &AuthSpy) -> TestResult<TcpStream> {
        let stream = TcpStream::connect_timeout(&spy.address(), Duration::from_secs(1))?;
        stream.set_read_timeout(Some(Duration::from_secs(1)))?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        Ok(stream)
    }

    #[test]
    fn should_answer_only_synthetic_refresh_and_join_spy() -> TestResult {
        let mut spy = AuthSpy::start()?;
        let mut stream = connect(&spy)?;
        let body = json!({
            "grant_type": "refresh_token", "client_id": "idle-client-canary",
            "client_secret": "idle-secret-canary", "refresh_token": "idle-refresh-canary",
            "scopes": "https://www.googleapis.com/auth/cloud-platform",
        })
        .to_string();
        write!(
            stream,
            "POST /token HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )?;
        let mut response = String::new();
        stream.take(4096).read_to_string(&mut response)?;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(response.contains("idle-access-canary"));
        assert_eq!(spy.requests(), 1);
        spy.close()?;
        assert!(spy.worker.is_none());
        Ok(())
    }

    #[test]
    fn should_reject_non_refresh_requests_without_echoing_payload() -> TestResult {
        let mut spy = AuthSpy::start()?;
        let mut stream = connect(&spy)?;
        stream.write_all(b"CONNECT private-provider-body HTTP/1.1\r\nHost: localhost\r\n\r\n")?;
        let mut response = Vec::new();
        stream.take(4096).read_to_end(&mut response)?;
        let result = spy.close();
        assert!(matches!(result, Err(ref error) if error.kind() == "IDLE_AUTH_REQUEST"));
        assert!(!format!("{result:?}").contains("private-provider-body"));
        assert!(response.is_empty());
        assert_eq!(spy.requests(), 0);
        assert!(spy.worker.is_none());
        Ok(())
    }

    #[test]
    fn should_bound_and_join_spy_with_incomplete_request() -> TestResult {
        let mut spy = AuthSpy::start()?;
        let mut stream = connect(&spy)?;
        stream.write_all(b"POST /token HTTP/1.1\r\n")?;
        let started = Instant::now();
        let mut response = Vec::new();
        stream.take(4096).read_to_end(&mut response)?;
        let result = spy.close();
        assert!(matches!(result, Err(error) if error.kind() == "IDLE_AUTH_REQUEST"));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(response.is_empty() && spy.worker.is_none());
        Ok(())
    }
}

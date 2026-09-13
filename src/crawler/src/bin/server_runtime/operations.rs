use super::lifecycle::{CrawlerState, Lifecycle};
use std::{io, net::SocketAddr, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

pub(super) struct OperationsServer {
    listener: TcpListener,
    lifecycle: Lifecycle,
    commit_sha: String,
}

impl OperationsServer {
    pub(super) async fn bind(
        addr: SocketAddr,
        lifecycle: Lifecycle,
        commit_sha: String,
    ) -> io::Result<Self> {
        // Defense in depth. No proxy/public/wildcard mode, even inside this private adapter.
        if !addr.ip().is_loopback() || addr.port() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "operations listener must use nonzero loopback",
            ));
        }
        Ok(Self {
            listener: TcpListener::bind(addr).await?,
            lifecycle,
            commit_sha,
        })
    }

    pub(super) async fn run_until(self, mut stop: watch::Receiver<bool>) -> io::Result<()> {
        let mut connections = JoinSet::new();
        let mut failure = None;
        loop {
            tokio::select! {
                biased;
                _ = stop.wait_for(|stop| *stop) => break,
                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = result {
                        failure = Some(io::Error::other(error));
                        self.lifecycle.stop(true);
                        break;
                    }
                }
                accepted = self.listener.accept(), if connections.len() < 16 => {
                    match accepted {
                        Ok((stream, _)) => {
                            let lifecycle = self.lifecycle.clone();
                            let sha = self.commit_sha.clone();
                            connections.spawn(async move {
                                // Probes own no domain work; peer I/O/slow headers are request-local.
                                let _peer_result = tokio::time::timeout(Duration::from_secs(1), serve(stream, lifecycle, sha)).await;
                            });
                        }
                        Err(error) => {
                            failure = Some(error);
                            self.lifecycle.stop(true);
                            break;
                        }
                    }
                }
            }
        }
        drop(self.listener);
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result {
                failure.get_or_insert_with(|| io::Error::other(error));
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

async fn serve(mut stream: TcpStream, lifecycle: Lifecycle, commit_sha: String) -> io::Result<()> {
    let mut request = [0_u8; 4096];
    let mut used = 0;
    loop {
        let read = stream.read(&mut request[used..]).await?;
        if read == 0 {
            return Ok(());
        }
        used += read;
        if request[..used].windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            break;
        }
        if used == request.len() {
            return Ok(());
        }
    }
    let line = request[..used]
        .split(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let state = lifecycle.state();
    let status = match line {
        b"GET /health HTTP/1.1\r" | b"GET /ops/version HTTP/1.1\r" => "200 OK",
        b"GET /ready HTTP/1.1\r" if state == CrawlerState::Ready => "200 OK",
        b"GET /ready HTTP/1.1\r" => "503 Service Unavailable",
        _ => "404 Not Found",
    };
    let body = serde_json::json!({ "commit_sha": commit_sha, "state": state.as_str() }).to_string();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
#[path = "operations_tests.rs"]
mod tests;

//! Test-only HTTP relay: capture a complete adapter request, then forward it unchanged in meaning
//! to real OpenSearch after the test has applied a newer projection. No source reread on release.
use opensearch::{
    OpenSearch,
    http::{Method, StatusCode, Url, headers::HeaderMap, request::JsonBody, transport::Transport},
};
use std::{io, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
    task::JoinHandle,
    time::timeout,
};

type TestError = Box<dyn std::error::Error + Send + Sync>;

pub(super) struct PausedWrite {
    pub(super) client: OpenSearch,
    received: oneshot::Receiver<()>,
    release: oneshot::Sender<()>,
    forwarded: JoinHandle<Result<StatusCode, TestError>>,
}

impl PausedWrite {
    pub(super) async fn new(target: OpenSearch) -> Result<Self, TestError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let client = OpenSearch::new(Transport::single_node(&format!(
            "http://{}",
            listener.local_addr()?
        ))?);
        let (received_tx, received) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let forwarded = tokio::spawn(async move {
            timeout(Duration::from_secs(120), async move {
                let (mut socket, _) = listener.accept().await?;
                let mut bytes = Vec::new();
                let (header_end, content_length) = loop {
                    let count = socket.read_buf(&mut bytes).await?;
                    if count == 0 || bytes.len() > 1_000_000 {
                        return Err(io::Error::other("incomplete or oversized test request").into());
                    }
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&bytes[..end])?;
                        let length = headers.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length").then_some(value.trim())
                        });
                        break (end + 4, length.unwrap_or("0").parse::<usize>()?);
                    }
                };
                while bytes.len() < header_end + content_length {
                    if socket.read_buf(&mut bytes).await? == 0 {
                        return Err(io::Error::other("incomplete test request body").into());
                    }
                }
                let headers = std::str::from_utf8(&bytes[..header_end])?;
                let mut request_line = headers.lines().next().unwrap_or_default().split_whitespace();
                let method = match request_line.next() {
                    Some("PUT") => Method::Put,
                    Some("POST") => Method::Post,
                    Some("DELETE") => Method::Delete,
                    _ => return Err(io::Error::other("expected a projection write").into()),
                };
                let path = request_line.next().ok_or_else(|| io::Error::other("missing path"))?;
                let url = Url::parse(&format!("http://relay.test{path}"))?;
                let query = url.query_pairs().into_owned().collect::<Vec<_>>();
                let body = (content_length != 0)
                    .then(|| serde_json::from_slice::<serde_json::Value>(&bytes[header_end..header_end + content_length]))
                    .transpose()?
                    .map(JsonBody::new);
                received_tx.send(()).map_err(|_| io::Error::other("test receiver dropped"))?;
                release_rx.await?;
                let response = target.send(method, url.path(), HeaderMap::new(), Some(&query), body, None).await?;
                let status = response.status_code();
                let body = response.text().await?;
                let reply = format!(
                    "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status.as_u16(), body.len(), body
                );
                socket.write_all(reply.as_bytes()).await?;
                socket.shutdown().await?;
                Ok(status)
            }).await?
        });
        Ok(Self {
            client,
            received,
            release,
            forwarded,
        })
    }

    pub(super) async fn wait_until_received(&mut self) -> Result<(), TestError> {
        if timeout(Duration::from_secs(10), &mut self.received)
            .await?
            .is_err()
        {
            return match (&mut self.forwarded).await? {
                Err(error) => Err(error),
                Ok(_) => Err(io::Error::other("relay ended before receiving a write").into()),
            };
        }
        Ok(())
    }

    pub(super) async fn resume(self) -> Result<StatusCode, TestError> {
        self.release
            .send(())
            .map_err(|_| io::Error::other("test relay dropped"))?;
        timeout(Duration::from_secs(30), self.forwarded).await??
    }
}

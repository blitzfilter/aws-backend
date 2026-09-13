use bloomfilter::Bloom;
use futures::FutureExt;
use reqwest::header::{ACCEPT_ENCODING, HeaderMap, HeaderValue};
use spider::page::{AntiBotTech, Page};
use spider::tokio;
use spider::utils::auto_throttle::AutoThrottleConfig;
use spider::website::{CrawlStatus, Website, WebsiteMetaInfo};
use std::{future::Future, panic::AssertUnwindSafe, time::Duration};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;
use url::Url;

use crate::network::policy::{is_same_or_www_host, resolve_public_http_target};
use crate::spider::utils::url::CrawledUrl;

const SPIDER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36";
const SPIDER_ACCEPT_ENCODING: &str = "gzip, br, deflate";
const MAX_ROOT_REDIRECTS: usize = 5;
const SPIDER_MAX_BODY_BYTES_ENV: &str = "SPIDER_MAX_SIZE_BYTES";
const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;

fn spider_request_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT_ENCODING,
        HeaderValue::from_static(SPIDER_ACCEPT_ENCODING),
    );
    headers
}

/// Single crawled page represented by its normalized URL.
#[derive(Debug, Clone)]
pub struct CrawledPage {
    pub url: CrawledUrl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlFailureKind {
    EmptyCrawl,
    RateLimited,
    AccessDenied,
    CloudflareChallenge,
    BotProtection,
    TlsError,
    ConnectError,
    ServerError,
    RedirectProblem,
    InvalidUrl,
    JavascriptRequired,
}

impl CrawlFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CrawlFailureKind::EmptyCrawl => "EmptyCrawl",
            CrawlFailureKind::RateLimited => "RateLimited",
            CrawlFailureKind::AccessDenied => "AccessDenied",
            CrawlFailureKind::CloudflareChallenge => "CloudflareChallenge",
            CrawlFailureKind::BotProtection => "BotProtection",
            CrawlFailureKind::TlsError => "TlsError",
            CrawlFailureKind::ConnectError => "ConnectError",
            CrawlFailureKind::ServerError => "ServerError",
            CrawlFailureKind::RedirectProblem => "RedirectProblem",
            CrawlFailureKind::InvalidUrl => "InvalidUrl",
            CrawlFailureKind::JavascriptRequired => "JavascriptRequired",
        }
    }
}

impl std::fmt::Display for CrawlFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct CrawlDiagnostics {
    pub failure_kind: Option<CrawlFailureKind>,
    pub http_status: Option<u16>,
    pub final_url: Option<String>,
    pub redirect_url: Option<String>,
    pub diagnostic_reason: Option<String>,
}

impl CrawlDiagnostics {
    fn apply_signal(&mut self, signal: DiagnosticSignal) {
        self.failure_kind = Some(signal.kind);
        self.diagnostic_reason = Some(signal.reason.to_string());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiagnosticSignal {
    kind: CrawlFailureKind,
    reason: &'static str,
}

impl DiagnosticSignal {
    const fn new(kind: CrawlFailureKind, reason: &'static str) -> Self {
        Self { kind, reason }
    }
}

/// Owns only our producer and forwarder, not Spider's internal network tasks.
/// Drop aborts both wrappers; use `cancel_and_join` to confirm their termination.
#[derive(Debug)]
#[must_use = "crawl tasks must be consumed to completion or cancelled and joined"]
pub struct SpiderCrawl {
    pages: mpsc::Receiver<CrawledPage>,
    diagnostics: oneshot::Receiver<CrawlDiagnostics>,
    tasks: JoinSet<Result<(), CrawlIncompleteError>>,
    failure: watch::Sender<Option<CrawlIncompleteError>>,
    outcome: Option<Result<CrawlDiagnostics, CrawlIncompleteError>>,
}

impl SpiderCrawl {
    fn spawn_owned<P, F>(
        pages: mpsc::Receiver<CrawledPage>,
        diagnostics: oneshot::Receiver<CrawlDiagnostics>,
        producer: P,
        forwarder: F,
        max_duration: Duration,
    ) -> Self
    where
        P: Future<Output = Result<(), CrawlIncompleteError>> + Send + 'static,
        F: Future<Output = Result<(), CrawlIncompleteError>> + Send + 'static,
    {
        let (failure, _) = watch::channel(None);
        let deadline = Instant::now() + max_duration;
        let mut tasks = JoinSet::new();
        tasks.spawn(run_owned_crawl_task(producer, failure.clone(), deadline));
        tasks.spawn(run_owned_crawl_task(forwarder, failure.clone(), deadline));
        Self {
            pages,
            diagnostics,
            tasks,
            failure,
            outcome: None,
        }
    }

    /// Cancellation-safe. `Ok(None)` confirms both wrapper joins and diagnostics.
    /// Pages already delivered before an error do not make the crawl complete.
    pub async fn recv(&mut self) -> Result<Option<CrawledPage>, SpiderDiscoveryError> {
        if let Some(Err(error)) = self.outcome {
            return Err(error.into());
        }
        let mut failure = self.failure.subscribe();
        tokio::select! {
            biased;
            _ = failure.wait_for(Option::is_some).map(drop) => {
                self.completion().await?;
                Ok(None)
            }
            page = self.pages.recv() => {
                if page.is_none() {
                    self.completion().await?;
                }
                Ok(page)
            }
        }
    }

    /// Joins both wrappers without consuming buffered pages. Normally call after
    /// `recv` returns `None`; undrained backpressure remains subject to the deadline.
    /// Cancellation-safe and repeatable, including after an incomplete result.
    pub async fn completion(&mut self) -> Result<CrawlDiagnostics, SpiderDiscoveryError> {
        if let Some(outcome) = &self.outcome {
            return outcome.clone().map_err(Into::into);
        }
        while let Some(result) = self.tasks.join_next().await {
            let result = result.unwrap_or_else(|error| {
                Err(if error.is_panic() {
                    CrawlIncompleteError::TaskPanicked
                } else {
                    CrawlIncompleteError::Cancelled
                })
            });
            if let Err(error) = result {
                record_crawl_failure(&self.failure, error);
                self.pages.close();
                self.tasks.abort_all();
            }
        }
        let outcome = match *self.failure.borrow() {
            Some(error) => Err(error),
            // Both writers have joined: no later diagnostics delivery is valid.
            None => self
                .diagnostics
                .try_recv()
                .map_err(|_| CrawlIncompleteError::MissingDiagnostics),
        };
        self.outcome = Some(outcome.clone());
        outcome.map_err(Into::into)
    }

    /// Stops and joins both wrappers. An unfinished crawl returns `Cancelled`
    /// (or its earlier failure), never success. A completed outcome stays unchanged.
    pub async fn cancel_and_join(&mut self) -> Result<CrawlDiagnostics, SpiderDiscoveryError> {
        if self.outcome.is_none() {
            record_crawl_failure(&self.failure, CrawlIncompleteError::Cancelled);
            self.pages.close();
            self.tasks.abort_all();
        }
        self.completion().await
    }

    #[cfg(test)]
    pub(crate) fn fixture(pages: Vec<CrawledPage>, diagnostics: Option<CrawlDiagnostics>) -> Self {
        let (tx, rx) = mpsc::channel(25);
        let (diagnostics_tx, diagnostics_rx) = oneshot::channel();
        Self::spawn_owned(
            rx,
            diagnostics_rx,
            async { Ok(()) },
            async move {
                for page in pages {
                    tx.send(page)
                        .await
                        .map_err(|_| CrawlIncompleteError::PageDeliveryClosed)?;
                }
                if let Some(diagnostics) = diagnostics {
                    diagnostics_tx
                        .send(diagnostics)
                        .map_err(|_| CrawlIncompleteError::DiagnosticsDeliveryClosed)?;
                }
                Ok(())
            },
            CrawlerConfig::default().max_crawl_duration,
        )
    }
}

impl Drop for SpiderCrawl {
    fn drop(&mut self) {
        self.tasks.abort_all();
    }
}

fn record_crawl_failure(
    failure: &watch::Sender<Option<CrawlIncompleteError>>,
    error: CrawlIncompleteError,
) {
    failure.send_if_modified(|current| {
        if current.is_none() {
            *current = Some(error);
            true
        } else {
            false
        }
    });
}

async fn run_owned_crawl_task(
    task: impl Future<Output = Result<(), CrawlIncompleteError>>,
    failure: watch::Sender<Option<CrawlIncompleteError>>,
    deadline: Instant,
) -> Result<(), CrawlIncompleteError> {
    let mut peer_failure = failure.subscribe();
    let result = tokio::select! {
        biased;
        _ = tokio::time::sleep_until(deadline) => Err(CrawlIncompleteError::DeadlineExceeded),
        // The original failure remains in the shared signal, not this cancellation.
        _ = peer_failure.wait_for(Option::is_some).map(drop) => Err(CrawlIncompleteError::Cancelled),
        result = AssertUnwindSafe(task).catch_unwind() => {
            match result {
                // A non-yielding final poll can outlive the timer check above.
                Ok(Ok(())) if Instant::now() >= deadline => Err(CrawlIncompleteError::DeadlineExceeded),
                Ok(result) => result,
                Err(_) => Err(CrawlIncompleteError::TaskPanicked),
            }
        }
    };
    if let Err(error) = result {
        record_crawl_failure(&failure, error);
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CrawlIncompleteError {
    #[error("page subscription lagged by {skipped} messages")]
    BroadcastLagged { skipped: u64 },
    #[error("producer status was not delivered")]
    MissingStatus,
    #[error("crawl diagnostics were not delivered")]
    MissingDiagnostics,
    #[error("page consumer closed")]
    PageDeliveryClosed,
    #[error("status consumer closed")]
    StatusDeliveryClosed,
    #[error("diagnostics consumer closed")]
    DiagnosticsDeliveryClosed,
    #[error("owned crawl task panicked")]
    TaskPanicked,
    #[error("owned crawl tasks cancelled")]
    Cancelled,
    #[error("crawl deadline exceeded")]
    DeadlineExceeded,
}

#[derive(Debug, Error)]
pub enum SpiderDiscoveryError {
    #[error("Spider discovery error: {0}")]
    Discovery(String),
    #[error("Spider crawl incomplete: {0}")]
    Incomplete(#[from] CrawlIncompleteError),
}

#[derive(Debug, Clone)]
pub struct CrawlerConfig {
    pub delay_millis: u64,
    pub request_timeout_secs: u64,
    pub concurrency_limit: usize,
    pub bloom_capacity: usize,
    pub bloom_fp_rate: f64,
    pub channel_size: usize,
    /// Maximum pages accepted in one crawl, including the root page.
    pub max_pages_per_crawl: u32,
    /// Producer/forwarder wall-clock budget after root preflight; requests have their own timeout.
    pub max_crawl_duration: std::time::Duration,
    /// Hard page-body ceiling enforced by Spider's streaming transport.
    pub max_response_body_bytes: usize,
}

impl Default for CrawlerConfig {
    fn default() -> Self {
        Self {
            delay_millis: 500,
            request_timeout_secs: 15,
            concurrency_limit: 8,
            bloom_capacity: 100_000,
            bloom_fp_rate: 0.001,
            channel_size: 1000,
            max_pages_per_crawl: 10_000,
            max_crawl_duration: std::time::Duration::from_secs(10 * 60),
            max_response_body_bytes: DEFAULT_MAX_RESPONSE_BODY_BYTES,
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait Spider: Send + Sync {
    async fn crawl(&self, crawl_root_url: &str) -> Result<SpiderCrawl, SpiderDiscoveryError>;
}

pub struct SpiderImpl {
    config: CrawlerConfig,
}

impl SpiderImpl {
    pub fn new(config: CrawlerConfig) -> Self {
        Self { config }
    }
}

impl Default for SpiderImpl {
    fn default() -> Self {
        Self::new(CrawlerConfig::default())
    }
}

fn configured_spider_body_limit(
    configured: Option<&str>,
    maximum: usize,
) -> Result<(), SpiderDiscoveryError> {
    let configured = configured
        .ok_or_else(|| {
            SpiderDiscoveryError::Discovery(format!(
                "{SPIDER_MAX_BODY_BYTES_ENV} must be set to the crawler page-body limit"
            ))
        })?
        .parse::<usize>()
        .map_err(|_| {
            SpiderDiscoveryError::Discovery(format!(
                "{SPIDER_MAX_BODY_BYTES_ENV} must be an integer page-body limit"
            ))
        })?;
    if !(1_048_576..=maximum).contains(&configured) {
        return Err(SpiderDiscoveryError::Discovery(format!(
            "{SPIDER_MAX_BODY_BYTES_ENV} must be between 1048576 and {maximum} bytes"
        )));
    }
    Ok(())
}

fn configured_host_whitelist(url: &Url) -> Result<String, SpiderDiscoveryError> {
    let host = url.host_str().ok_or_else(|| {
        SpiderDiscoveryError::Discovery("configured crawler URL has no host".to_string())
    })?;
    Ok(format!(
        r"^https?://{}(?::(?:80|443))?(?:/|$)",
        regex::escape(host)
    ))
}

async fn spider_public_http_client(
    url: &Url,
    timeout: std::time::Duration,
) -> Result<spider::reqwest::Client, SpiderDiscoveryError> {
    let target = resolve_public_http_target(url, timeout)
        .await
        .map_err(|error| SpiderDiscoveryError::Discovery(error.to_string()))?;
    let mut builder = spider::reqwest::Client::builder()
        .redirect(spider::reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .no_proxy();
    for address in target.addresses {
        builder = builder.resolve(&target.host, address);
    }
    builder
        .build()
        .map_err(|error| SpiderDiscoveryError::Discovery(error.to_string()))
}

fn root_redirect_target(
    configured_root: &Url,
    current_root: &Url,
    location: &str,
) -> Result<Url, SpiderDiscoveryError> {
    let redirect_target = current_root.join(location).map_err(|_| {
        SpiderDiscoveryError::Discovery("crawl-root redirect location is invalid".to_string())
    })?;

    if !is_same_or_www_host(configured_root, &redirect_target) {
        return Err(SpiderDiscoveryError::Discovery(
            "crawl-root redirect target is outside the configured bare/www host".to_string(),
        ));
    }

    Ok(redirect_target)
}

async fn preflight_crawl_root(
    configured_root: Url,
    timeout: std::time::Duration,
) -> Result<Url, SpiderDiscoveryError> {
    let mut current_root = configured_root.clone();

    for redirect_count in 0..=MAX_ROOT_REDIRECTS {
        let client = spider_public_http_client(&current_root, timeout).await?;
        let response = client
            .get(current_root.clone())
            .send()
            .await
            .map_err(|error| SpiderDiscoveryError::Discovery(error.to_string()))?;

        if !response.status().is_redirection() {
            return Ok(current_root);
        }

        if redirect_count == MAX_ROOT_REDIRECTS {
            return Err(SpiderDiscoveryError::Discovery(
                "crawl-root redirect limit exceeded".to_string(),
            ));
        }

        let location = response
            .headers()
            .get("location")
            .ok_or_else(|| {
                SpiderDiscoveryError::Discovery(
                    "crawl-root redirect response has no location".to_string(),
                )
            })?
            .to_str()
            .map_err(|_| {
                SpiderDiscoveryError::Discovery(
                    "crawl-root redirect location is invalid".to_string(),
                )
            })?;
        current_root = root_redirect_target(&configured_root, &current_root, location)?;
    }

    Err(SpiderDiscoveryError::Discovery(
        "crawl-root redirect limit exceeded".to_string(),
    ))
}

#[async_trait::async_trait]
impl Spider for SpiderImpl {
    async fn crawl(&self, crawl_root_url: &str) -> Result<SpiderCrawl, SpiderDiscoveryError> {
        configured_spider_body_limit(
            std::env::var(SPIDER_MAX_BODY_BYTES_ENV).ok().as_deref(),
            self.config.max_response_body_bytes,
        )?;
        let (tx, rx) = mpsc::channel(self.config.channel_size);
        let (status_tx, status_rx) = oneshot::channel();
        let (diagnostics_tx, diagnostics_rx) = oneshot::channel();

        let configured_root = Url::parse(crawl_root_url).map_err(|_| {
            SpiderDiscoveryError::Discovery("configured crawler URL is invalid".to_string())
        })?;
        let request_timeout = std::time::Duration::from_secs(self.config.request_timeout_secs);
        let root_url = preflight_crawl_root(configured_root, request_timeout).await?;
        let client = spider_public_http_client(&root_url, request_timeout).await?;
        let host_whitelist = configured_host_whitelist(&root_url)?;
        let mut website = Website::new(root_url.as_str());
        website.set_http_client(client);

        let blacklist_regex = CrawledUrl::blacklist_patterns();
        let auto_throttle_config = AutoThrottleConfig {
            min_delay_ms: self.config.delay_millis,
            max_delay_ms: 60_000,
            ..AutoThrottleConfig::default()
        };
        website
            .configuration
            .with_auto_throttle(auto_throttle_config);
        website
            .configuration
            .with_concurrency_limit(Some(self.config.concurrency_limit.max(1)));

        website
            .with_blacklist_url(Some(blacklist_regex))
            .with_whitelist_url(Some(vec![host_whitelist.into()]))
            .with_respect_robots_txt(true)
            .with_headers(Some(spider_request_headers()))
            .with_user_agent(Some(SPIDER_USER_AGENT))
            .with_request_timeout(Some(std::time::Duration::from_secs(
                self.config.request_timeout_secs,
            )))
            // Spider's own timeout returns unit and can masquerade as success.
            // Our owned wrappers enforce the same budget with a typed failure.
            .with_crawl_timeout(None)
            .with_limit(self.config.max_pages_per_crawl)
            .with_delay(
                std::time::Duration::from_millis(self.config.delay_millis).as_millis() as u64,
            )
            .with_caching(false)
            .build()
            .map_err(|_| SpiderDiscoveryError::Discovery("Failed to build website".to_string()))?;

        let spider_rx = website.subscribe(512);
        let bloom = Bloom::new_for_fp_rate(self.config.bloom_capacity, self.config.bloom_fp_rate)
            .map_err(|_| {
            SpiderDiscoveryError::Discovery("bloom filter init failed".to_string())
        })?;
        let producer = async move {
            website.crawl().await;
            let status = *website.get_status();
            let meta = *website.get_website_meta_info();
            website.unsubscribe();
            status_tx
                .send((status, meta))
                .map_err(|_| CrawlIncompleteError::StatusDeliveryClosed)
        };
        let forwarder = forward_pages(spider_rx, status_rx, tx, diagnostics_tx, root_url, bloom);

        Ok(SpiderCrawl::spawn_owned(
            rx,
            diagnostics_rx,
            producer,
            forwarder,
            self.config.max_crawl_duration,
        ))
    }
}

async fn forward_pages(
    mut spider_rx: broadcast::Receiver<Page>,
    status_rx: oneshot::Receiver<(CrawlStatus, WebsiteMetaInfo)>,
    tx: mpsc::Sender<CrawledPage>,
    diagnostics_tx: oneshot::Sender<CrawlDiagnostics>,
    configured_root: Url,
    mut bloom: Bloom<String>,
) -> Result<(), CrawlIncompleteError> {
    let forwarding = async {
        let mut diagnostics = CrawlDiagnostics::default();
        let mut first_page_seen = false;
        let mut root_redirect_rejected = false;

        loop {
            let page = match spider_rx.recv().await {
                Ok(page) => page,
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    return Err(CrawlIncompleteError::BroadcastLagged { skipped });
                }
            };
            if !first_page_seen {
                diagnostics = diagnostics_from_library_page(
                    configured_root.as_str(),
                    page.get_url(),
                    page.status_code.as_u16(),
                    page.final_redirect_destination.as_deref(),
                    page.anti_bot_tech,
                );
                root_redirect_rejected = matches!(
                    diagnostics.failure_kind,
                    Some(CrawlFailureKind::RedirectProblem)
                );
                first_page_seen = true;
            }

            let normalized = if let Ok(parsed) = Url::parse(page.get_url()) {
                CrawledUrl::new(parsed)
            } else {
                continue;
            };
            if root_redirect_rejected
                || !is_same_or_www_host(&configured_root, normalized.as_url())
                || normalized.is_blacklisted()
            {
                continue;
            }
            let normalized_str = normalized.to_string();
            if !bloom.check(&normalized_str) {
                bloom.set(&normalized_str);
                tx.send(CrawledPage { url: normalized })
                    .await
                    .map_err(|_| CrawlIncompleteError::PageDeliveryClosed)?;
            }
        }

        let (status, meta) = status_rx
            .await
            .map_err(|_| CrawlIncompleteError::MissingStatus)?;
        apply_website_status(&mut diagnostics, status, meta);
        diagnostics_tx
            .send(diagnostics)
            .map_err(|_| CrawlIncompleteError::DiagnosticsDeliveryClosed)
    };
    tokio::select! {
        biased;
        _ = tx.closed() => Err(CrawlIncompleteError::PageDeliveryClosed),
        result = forwarding => result,
    }
}

fn diagnostics_from_library_page(
    crawl_root_url: &str,
    page_url: &str,
    status_code: u16,
    final_redirect_destination: Option<&str>,
    anti_bot_tech: AntiBotTech,
) -> CrawlDiagnostics {
    let final_url = final_redirect_destination.unwrap_or(page_url).to_string();
    let redirect_url = (final_redirect_destination != Some(page_url)).then(|| final_url.clone());
    let mut diagnostics = CrawlDiagnostics {
        http_status: Some(status_code),
        final_url: Some(final_url.clone()),
        redirect_url,
        ..CrawlDiagnostics::default()
    };

    if let Some(signal) =
        page_diagnostic_signal(crawl_root_url, &final_url, status_code, anti_bot_tech)
    {
        diagnostics.apply_signal(signal);
    }

    diagnostics
}

fn page_diagnostic_signal(
    crawl_root_url: &str,
    final_url: &str,
    status_code: u16,
    anti_bot_tech: AntiBotTech,
) -> Option<DiagnosticSignal> {
    anti_bot_signal(anti_bot_tech)
        .or_else(|| status_code_signal(status_code))
        .or_else(|| redirect_signal(crawl_root_url, final_url))
}

fn anti_bot_signal(anti_bot_tech: AntiBotTech) -> Option<DiagnosticSignal> {
    match anti_bot_tech {
        AntiBotTech::Cloudflare => Some(DiagnosticSignal::new(
            CrawlFailureKind::CloudflareChallenge,
            "library_cloudflare_antibot",
        )),
        AntiBotTech::None => None,
        _ => Some(DiagnosticSignal::new(
            CrawlFailureKind::BotProtection,
            "library_bot_protection_antibot",
        )),
    }
}

fn status_code_signal(status_code: u16) -> Option<DiagnosticSignal> {
    match status_code {
        526 => Some(DiagnosticSignal::new(
            CrawlFailureKind::TlsError,
            "library_permanent_address_or_tls_error",
        )),
        525 => Some(DiagnosticSignal::new(
            CrawlFailureKind::ConnectError,
            "library_dns_or_connect_error",
        )),
        310 => Some(DiagnosticSignal::new(
            CrawlFailureKind::RedirectProblem,
            "library_too_many_redirects",
        )),
        _ => None,
    }
}

fn redirect_signal(crawl_root_url: &str, final_url: &str) -> Option<DiagnosticSignal> {
    let original = Url::parse(crawl_root_url).ok()?;
    let resolved = Url::parse(final_url).ok()?;

    (!is_same_or_www_host(&original, &resolved)).then_some(DiagnosticSignal::new(
        CrawlFailureKind::RedirectProblem,
        "library_redirect_to_unrelated_host",
    ))
}

fn apply_website_status(
    diagnostics: &mut CrawlDiagnostics,
    status: CrawlStatus,
    meta: WebsiteMetaInfo,
) {
    if diagnostics.failure_kind.is_some() {
        return;
    }

    if let Some(signal) = website_status_signal(status, meta) {
        diagnostics.apply_signal(signal);
    }
}

fn website_status_signal(status: CrawlStatus, meta: WebsiteMetaInfo) -> Option<DiagnosticSignal> {
    match (status, meta) {
        (CrawlStatus::RateLimited, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::RateLimited,
            "library_rate_limited_status",
        )),
        (CrawlStatus::Blocked, WebsiteMetaInfo::RequiresJavascript) => Some(DiagnosticSignal::new(
            CrawlFailureKind::JavascriptRequired,
            "library_requires_javascript",
        )),
        (CrawlStatus::Blocked, _) | (CrawlStatus::FirewallBlocked, _) => Some(
            DiagnosticSignal::new(CrawlFailureKind::AccessDenied, "library_blocked_status"),
        ),
        (CrawlStatus::Empty, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::EmptyCrawl,
            "library_empty_status",
        )),
        (CrawlStatus::ConnectError, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::ConnectError,
            "library_connect_error_status",
        )),
        (CrawlStatus::ServerError, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::ServerError,
            "library_server_error_status",
        )),
        (CrawlStatus::Invalid, _) => Some(DiagnosticSignal::new(
            CrawlFailureKind::InvalidUrl,
            "library_invalid_url_status",
        )),
        _ => None,
    }
}

#[cfg(test)]
#[path = "website_spider_tests.rs"]
mod ownership_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_allow_only_configured_host_in_spider_fetch_graph() {
        let pattern =
            configured_host_whitelist(&Url::parse("https://example.com").unwrap()).unwrap();
        let whitelist = regex::Regex::new(&pattern).unwrap();

        assert!(whitelist.is_match("https://example.com/product/1"));
        assert!(!whitelist.is_match("https://www.example.com/product/1"));
        assert!(!whitelist.is_match("https://internal.example.com/product/1"));
        assert!(!whitelist.is_match("https://example.com.evil.test/product/1"));
        assert!(!whitelist.is_match("http://127.0.0.1/product/1"));
    }

    #[test]
    fn should_build_exact_www_host_graph_after_www_root_preflight() {
        let pattern =
            configured_host_whitelist(&Url::parse("https://www.example.com/").unwrap()).unwrap();
        let whitelist = regex::Regex::new(&pattern).unwrap();

        assert!(whitelist.is_match("https://www.example.com/product/1"));
        assert!(!whitelist.is_match("https://example.com/product/1"));
    }

    #[test]
    fn should_resolve_relative_root_redirect_on_configured_host() {
        let configured = Url::parse("https://www.example.com/catalog").unwrap();
        let target = root_redirect_target(&configured, &configured, "/").unwrap();

        assert_eq!(target.as_str(), "https://www.example.com/");
    }

    #[test]
    fn should_allow_root_redirect_from_bare_host_to_www_host() {
        let configured = Url::parse("https://example.com/catalog").unwrap();
        let target =
            root_redirect_target(&configured, &configured, "https://www.example.com/").unwrap();

        assert_eq!(target.as_str(), "https://www.example.com/");
    }

    #[test]
    fn should_reject_root_redirect_to_unrelated_host() {
        let configured = Url::parse("https://example.com/catalog").unwrap();
        let error =
            root_redirect_target(&configured, &configured, "https://other.example/").unwrap_err();

        assert!(
            error
                .to_string()
                .contains("outside the configured bare/www host")
        );
    }

    #[test]
    fn should_bound_root_redirects() {
        assert_eq!(MAX_ROOT_REDIRECTS, 5);
    }

    #[test]
    fn should_require_a_bounded_spider_transport_body_limit() {
        assert!(configured_spider_body_limit(Some("8388608"), 8 * 1024 * 1024).is_ok());
        assert!(configured_spider_body_limit(None, 8 * 1024 * 1024).is_err());
        assert!(configured_spider_body_limit(Some("not-a-number"), 8 * 1024 * 1024).is_err());
        assert!(configured_spider_body_limit(Some("8388609"), 8 * 1024 * 1024).is_err());
    }

    #[test]
    fn should_use_conservative_website_concurrency_limit_by_default() {
        let config = CrawlerConfig::default();

        assert_eq!(config.concurrency_limit, 8);
        assert_eq!(config.max_pages_per_crawl, 10_000);
        assert_eq!(config.max_response_body_bytes, 8 * 1024 * 1024);
        assert_eq!(
            config.max_crawl_duration,
            std::time::Duration::from_secs(10 * 60)
        );
    }

    fn page_diagnostics(url: &str, status_code: u16) -> CrawlDiagnostics {
        diagnostics_from_library_page(
            "https://example.com",
            url,
            status_code,
            None,
            AntiBotTech::None,
        )
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_product_path() {
        let page = CrawledPage {
            url: CrawledUrl::new(url::Url::parse("https://example.com/product/1").unwrap()),
        };

        assert_eq!(page.url.to_string(), "https://example.com/product/1");
    }

    #[test]
    fn should_store_url_when_creating_crawled_page_for_non_product_path() {
        let page = CrawledPage {
            url: CrawledUrl::new(url::Url::parse("https://example.com/about").unwrap()),
        };

        assert_eq!(page.url.to_string(), "https://example.com/about");
    }

    #[test]
    fn should_not_request_zstd_when_building_spider_headers() {
        let headers = spider_request_headers();
        let accept_encoding = headers
            .get(reqwest::header::ACCEPT_ENCODING)
            .and_then(|value| value.to_str().ok());

        assert_eq!(accept_encoding, Some("gzip, br, deflate"));
    }

    #[test]
    fn should_accept_redirect_from_bare_host_to_www_host() {
        let original = Url::parse("https://moeblinger.de").unwrap();
        let resolved = Url::parse("https://www.moeblinger.de/").unwrap();

        assert!(is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_accept_redirect_from_www_host_to_bare_host() {
        let original = Url::parse("https://www.example.com").unwrap();
        let resolved = Url::parse("https://example.com/").unwrap();

        assert!(is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_reject_redirect_to_other_subdomain() {
        let original = Url::parse("https://example.com").unwrap();
        let resolved = Url::parse("https://catalog.example.com/").unwrap();

        assert!(!is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_reject_redirect_to_other_domain() {
        let original = Url::parse("https://example.com").unwrap();
        let resolved = Url::parse("https://other.com/").unwrap();

        assert!(!is_same_or_www_host(&original, &resolved));
    }

    #[test]
    fn should_map_rate_limited_library_status() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::RateLimited,
            WebsiteMetaInfo::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::RateLimited)
        );
    }

    #[test]
    fn should_map_blocked_library_status_to_access_denied() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::Blocked,
            WebsiteMetaInfo::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::AccessDenied)
        );
    }

    #[test]
    fn should_map_javascript_required_library_metadata() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::Blocked,
            WebsiteMetaInfo::RequiresJavascript,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::JavascriptRequired)
        );
    }

    #[test]
    fn should_map_empty_library_status_to_empty_crawl() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(&mut diagnostics, CrawlStatus::Empty, WebsiteMetaInfo::None);

        assert_eq!(diagnostics.failure_kind, Some(CrawlFailureKind::EmptyCrawl));
    }

    #[test]
    fn should_map_connect_error_library_status() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::ConnectError,
            WebsiteMetaInfo::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::ConnectError)
        );
    }

    #[test]
    fn should_map_server_error_library_status() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::ServerError,
            WebsiteMetaInfo::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::ServerError)
        );
    }

    #[test]
    fn should_map_invalid_library_status_to_invalid_url() {
        let mut diagnostics = CrawlDiagnostics::default();

        apply_website_status(
            &mut diagnostics,
            CrawlStatus::Invalid,
            WebsiteMetaInfo::None,
        );

        assert_eq!(diagnostics.failure_kind, Some(CrawlFailureKind::InvalidUrl));
    }

    #[test]
    fn should_map_cloudflare_antibot_page() {
        let diagnostics = diagnostics_from_library_page(
            "https://example.com",
            "https://example.com/",
            403,
            None,
            AntiBotTech::Cloudflare,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::CloudflareChallenge)
        );
    }

    #[test]
    fn should_map_non_cloudflare_antibot_page_to_bot_protection() {
        let diagnostics = diagnostics_from_library_page(
            "https://example.com",
            "https://example.com/",
            403,
            None,
            AntiBotTech::DataDome,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::BotProtection)
        );
    }

    #[test]
    fn should_not_map_cloudflare_from_normal_page_without_library_antibot_signal() {
        let diagnostics = page_diagnostics("https://example.com/cloudflare-cdn", 200);

        assert_eq!(diagnostics.failure_kind, None);
    }

    #[test]
    fn should_map_library_final_redirect_to_other_domain() {
        let diagnostics = diagnostics_from_library_page(
            "https://example.com",
            "https://example.com/",
            200,
            Some("https://other.com/"),
            AntiBotTech::None,
        );

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::RedirectProblem)
        );
    }

    #[test]
    fn should_map_library_permanent_address_status_to_tls_error() {
        let diagnostics = page_diagnostics("https://example.com/", 526);

        assert_eq!(diagnostics.failure_kind, Some(CrawlFailureKind::TlsError));
    }

    #[test]
    fn should_map_library_dns_status_to_connect_error() {
        let diagnostics = page_diagnostics("https://example.com/", 525);

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::ConnectError)
        );
    }

    #[test]
    fn should_map_library_too_many_redirects_status_to_redirect_problem() {
        let diagnostics = page_diagnostics("https://example.com/", 310);

        assert_eq!(
            diagnostics.failure_kind,
            Some(CrawlFailureKind::RedirectProblem)
        );
    }
}

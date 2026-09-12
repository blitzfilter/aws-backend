use crate::review::assets::{
    APP_JS, INDEX_HTML, STYLES_CSS, instrument_live_html, instrument_review_page,
};
use crate::review::http::{HttpResponse, ParsedRequest, parse_request};
use crate::review::model::{CrawlerReviewPage, SchemaMatrix};
use crate::review::repository::{CrawlerReviewRepository, ReviewRepositoryError};
use crate::scraper::css_selector::rule::ExtractionRule;
use crate::scraper::scraper_service::service::{FetchError, HtmlFetcher, ReqwestHtmlFetcher};
use crate::service::crawler_domain_configuration::{
    CrawlerDomainAdministration, CrawlerDomainConfigurationError,
};
use crate::{CrawlerDomainId, CrawlerReviewId, CrawlerReviewPageId};
use dashmap::DashMap;
use listing_source_core::{Domain, ListingSourceId};
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinError, JoinSet};
use tokio::time::Instant;
use tracing::{error, info, warn};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum ReviewServerConfigError {
    #[error(transparent)]
    InvalidBindAddress(#[from] std::net::AddrParseError),
    #[error("CRAWLER_REVIEW_AUTH_TOKEN is required for non-loopback bind addresses")]
    AuthenticationRequiredForNonLoopback,
}

#[derive(Clone)]
pub struct ReviewServerConfig {
    pub bind_addr: SocketAddr,
    pub auth_token: Option<String>,
}

impl ReviewServerConfig {
    pub fn from_env() -> Result<Self, ReviewServerConfigError> {
        let bind_addr: SocketAddr = std::env::var("CRAWLER_REVIEW_BIND_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:7878".to_string())
            .parse()?;
        let auth_token = std::env::var("CRAWLER_REVIEW_AUTH_TOKEN")
            .ok()
            .filter(|token| !token.trim().is_empty());
        Self {
            bind_addr,
            auth_token,
        }
        .validate()
    }

    pub fn validate(self) -> Result<Self, ReviewServerConfigError> {
        if !self.bind_addr.ip().is_loopback() && self.auth_token.is_none() {
            return Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback);
        }
        Ok(self)
    }
}

const MAX_CONCURRENT_CONNECTIONS: usize = 32;
const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
const REQUEST_IO_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
const RESPONSE_WRITE_DEADLINE: Duration = Duration::from_secs(10);
const REVIEW_SESSION_TTL: Duration = Duration::from_secs(8 * 60 * 60);
const REVIEW_SESSION_COOKIE: &str = "crawler_review_session";

#[derive(Clone)]
pub struct ReviewServer {
    repository: CrawlerReviewRepository,
    domain_administration: Arc<dyn CrawlerDomainAdministration>,
    config: ReviewServerConfig,
    html_fetcher: Arc<dyn HtmlFetcher>,
    sessions: Arc<DashMap<uuid::Uuid, std::time::Instant>>,
}

/// Owns the listener before the runtime announces readiness.
#[must_use]
pub struct BoundReviewServer {
    server: ReviewServer,
    listener: TcpListener,
}

impl BoundReviewServer {
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Stop admission, drain to one absolute deadline, then abort and join survivors.
    /// The caller must await this future; outer runtime/watchdog ownership stays with it.
    pub async fn run_until<F>(self, shutdown: F, drain: Duration) -> std::io::Result<()>
    where
        F: Future<Output = ()>,
    {
        let Self { server, listener } = self;
        let mut connections = JoinSet::new();
        let result = server
            .accept_until(|| listener.accept(), shutdown, &mut connections)
            .await;
        let stopped_at = Instant::now();
        drop(listener);
        let (deadline, result) = match stopped_at.checked_add(drain) {
            Some(deadline) => (deadline, result),
            None => (
                stopped_at,
                result.and(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "review drain duration is too large",
                ))),
            ),
        };
        drain_connections(&mut connections, deadline, result).await
    }
}

impl ReviewServer {
    pub fn new(
        repository: CrawlerReviewRepository,
        domain_administration: Arc<dyn CrawlerDomainAdministration>,
        config: ReviewServerConfig,
    ) -> Self {
        Self {
            repository,
            domain_administration,
            config,
            html_fetcher: Arc::new(ReqwestHtmlFetcher::new()),
            sessions: Arc::new(DashMap::new()),
        }
    }

    pub fn new_with_fetcher(
        repository: CrawlerReviewRepository,
        domain_administration: Arc<dyn CrawlerDomainAdministration>,
        config: ReviewServerConfig,
        html_fetcher: Arc<dyn HtmlFetcher>,
    ) -> Self {
        Self {
            repository,
            domain_administration,
            config,
            html_fetcher,
            sessions: Arc::new(DashMap::new()),
        }
    }

    pub async fn run(self) -> std::io::Result<()> {
        self.run_until(
            std::future::pending(),
            REQUEST_DEADLINE + RESPONSE_WRITE_DEADLINE,
        )
        .await
    }

    /// Bind using the configured address and auth policy, before publishing readiness.
    pub async fn bind(self) -> std::io::Result<BoundReviewServer> {
        self.config
            .clone()
            .validate()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        info!(
            bind_addr = %listener.local_addr()?,
            "Crawler review console listening"
        );
        Ok(BoundReviewServer {
            server: self,
            listener,
        })
    }

    /// Returns failure on accept/connection task failure or an incomplete drain.
    /// Signal shutdown and await completion; dropping this future cannot join tasks.
    pub async fn run_until<F>(self, shutdown: F, drain: Duration) -> std::io::Result<()>
    where
        F: Future<Output = ()>,
    {
        self.bind().await?.run_until(shutdown, drain).await
    }

    async fn accept_until<F, A, R>(
        &self,
        mut accept: A,
        shutdown: F,
        connections: &mut JoinSet<std::io::Result<()>>,
    ) -> std::io::Result<()>
    where
        F: Future<Output = ()>,
        A: FnMut() -> R,
        R: Future<Output = std::io::Result<(TcpStream, SocketAddr)>>,
    {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => return Ok(()),
                Some(result) = connections.join_next(), if !connections.is_empty() => {
                    connection_result(result)?;
                }
                // Count retained tasks, not only running tasks: completed handles stay bounded too.
                accepted = async { accept().await }, if connections.len() < MAX_CONCURRENT_CONNECTIONS => {
                    let (stream, _) = accepted?;
                    let server = self.clone();
                    connections.spawn(async move { server.handle_connection(stream).await });
                }
            }
        }
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> std::io::Result<()> {
        let Some(response) = self
            .response_for_request(&mut stream, REQUEST_DEADLINE)
            .await
        else {
            return Ok(());
        };
        let response = response.to_string();
        write_response(&mut stream, response.as_bytes(), RESPONSE_WRITE_DEADLINE).await
    }

    async fn response_for_request<R>(
        &self,
        stream: &mut R,
        deadline: Duration,
    ) -> Option<HttpResponse>
    where
        R: AsyncRead + Unpin,
    {
        match tokio::time::timeout(deadline, async {
            match read_bounded_request_headers(stream).await {
                Ok(Some(headers)) => {
                    let Some(request) = parse_request(&headers.head) else {
                        return Some(HttpResponse::text(400, "bad request"));
                    };
                    if !self.request_is_authorized(&request) {
                        return Some(HttpResponse::json(401, &json!({ "error": "unauthorized" })));
                    }
                    match read_bounded_request_body(stream, headers).await {
                        Ok(request) => Some(self.route(&request).await),
                        Err(error) => {
                            warn!(error = ?error, "Rejected malformed review console request");
                            Some(HttpResponse::json(400, &json!({ "error": "bad request" })))
                        }
                    }
                }
                Ok(None) => None,
                Err(error) => {
                    warn!(error = ?error, "Rejected malformed review console request");
                    Some(HttpResponse::json(400, &json!({ "error": "bad request" })))
                }
            }
        })
        .await
        {
            Ok(response) => response,
            Err(_) => {
                warn!("Review console request deadline exceeded");
                Some(HttpResponse::json(
                    400,
                    &json!({ "error": "request timed out" }),
                ))
            }
        }
    }

    async fn route(&self, request: &str) -> HttpResponse {
        let Some(parsed) = parse_request(request) else {
            return HttpResponse::text(400, "bad request");
        };

        if !self.request_is_authorized(&parsed) {
            return HttpResponse::json(401, &json!({ "error": "unauthorized" }));
        }

        match (parsed.method.as_str(), parsed.path) {
            ("GET", "/") => HttpResponse::html(200, INDEX_HTML),
            ("GET", "/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("GET", "/assets/app.js") => HttpResponse::javascript(200, APP_JS),
            ("GET", "/assets/styles.css") => HttpResponse::css(200, STYLES_CSS),
            ("GET", "/api/health") => HttpResponse::json(200, &json!({ "ok": true })),
            ("POST", "/api/session") => self.create_session(),
            ("POST", "/api/session/logout") => self.clear_session(&parsed),
            ("GET", "/api/listing_sources") => {
                match self.repository.list_listing_sources(200).await {
                    Ok(listing_sources) => HttpResponse::json(200, &listing_sources),
                    Err(err) => internal_error(err),
                }
            }
            ("GET", "/api/reviews") => match self.repository.list_reviews(200).await {
                Ok(reviews) => HttpResponse::json(200, &reviews),
                Err(err) => internal_error(err),
            },
            _ => self.route_dynamic(parsed).await,
        }
    }

    async fn route_dynamic(&self, request: ParsedRequest<'_>) -> HttpResponse {
        if request.method == "GET"
            && request.path.starts_with("/api/listing-sources/")
            && request.path.ends_with("/domains")
        {
            let Some(listing_source_id) =
                parse_listing_source_id_with_suffix(request.path, "/domains")
            else {
                return HttpResponse::json(400, &json!({ "error": "invalid ListingSource id" }));
            };
            return match self
                .domain_administration
                .list_crawler_domains(listing_source_id)
                .await
            {
                Ok(domains) => HttpResponse::json(
                    200,
                    &domains
                        .into_iter()
                        .map(|domain| {
                            json!({
                                "domain_id": domain.domain_id,
                                "listing_source_id": domain.listing_source_id,
                                "domain": domain.domain.as_str(),
                            })
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(error) => domain_configuration_error(error),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/listing-sources/")
            && request.path.ends_with("/domains")
        {
            let Some(listing_source_id) =
                parse_listing_source_id_with_suffix(request.path, "/domains")
            else {
                return HttpResponse::json(400, &json!({ "error": "invalid ListingSource id" }));
            };
            let payload: DomainPayload = match serde_json::from_str(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            let domain = match Domain::try_from(payload.domain) {
                Ok(domain) => domain,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .domain_administration
                .register_crawler_domain(listing_source_id, domain)
                .await
            {
                Ok(domain) => HttpResponse::json(
                    if domain.created { 201 } else { 200 },
                    &json!({
                        "domain_id": domain.domain_id,
                        "listing_source_id": domain.listing_source_id,
                        "domain": domain.domain.as_str(),
                        "created": domain.created,
                    }),
                ),
                Err(error) => domain_configuration_error(error),
            };
        }

        if request.method == "DELETE"
            && request.path.starts_with("/api/listing-sources/")
            && request.path.contains("/domains/")
        {
            let Some((listing_source_id, domain_id)) = parse_listing_source_domain_id(request.path)
            else {
                return HttpResponse::json(
                    400,
                    &json!({ "error": "invalid ListingSource or domain id" }),
                );
            };
            return match self
                .domain_administration
                .remove_crawler_domain(listing_source_id, domain_id)
                .await
            {
                Ok(removal) => HttpResponse::json(
                    200,
                    &json!({
                        "domain_id": removal.domain_id,
                        "removed_url_count": removal.removed_url_count,
                        "removed_url_pattern_review_count": removal.removed_url_pattern_review_count,
                    }),
                ),
                Err(error) => domain_configuration_error(error),
            };
        }

        if request.method == "GET" && request.path == "/api/live-inspect" {
            let Some(url) = request.query.get("url") else {
                return HttpResponse::json(400, &json!({ "error": "missing url" }));
            };
            return match self.fetch_live_url(url).await {
                Ok(html) => HttpResponse::html(200, &instrument_live_html(url, &html)),
                Err(response) => response,
            };
        }

        if request.method == "GET" && request.path == "/api/live-html" {
            let Some(url) = request.query.get("url") else {
                return HttpResponse::json(400, &json!({ "error": "missing url" }));
            };
            return match self.fetch_live_url(url).await {
                Ok(html) => HttpResponse::text(200, &html),
                Err(response) => response,
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/matrix")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/matrix") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let refresh = request
                .query
                .get("refresh")
                .is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
            if !refresh {
                match self.cached_schema_matrix(review_id).await {
                    Ok(Some(matrix)) => return HttpResponse::json(200, &matrix),
                    Ok(None) => {}
                    Err(err) => return internal_error(err),
                }
            }
            return match self.refresh_schema_matrix(review_id).await {
                Ok(matrix) => HttpResponse::json(200, &matrix),
                Err(response) => response,
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/inspect")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/inspect") else {
                return HttpResponse::text(400, "invalid page id");
            };
            return match self.live_review_page(page_id).await {
                Ok(Some((page, html))) => {
                    HttpResponse::html(200, &instrument_review_page(&page, &html))
                }
                Ok(None) => HttpResponse::text(404, "not found"),
                Err(response) => response,
            };
        }

        if request.method == "GET"
            && request.path.starts_with("/api/review-pages/")
            && request.path.ends_with("/html")
        {
            let Some(page_id) = parse_page_id_with_suffix(request.path, "/html") else {
                return HttpResponse::text(400, "invalid page id");
            };
            return match self.live_review_page(page_id).await {
                Ok(Some((_page, html))) => HttpResponse::text(200, &html),
                Ok(None) => HttpResponse::text(404, "not found"),
                Err(response) => response,
            };
        }

        if request.method == "GET" && request.path.starts_with("/api/reviews/") {
            let Some(review_id) = parse_review_id(request.path) else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            return match self.repository.get_review(review_id).await {
                Ok(detail) => HttpResponse::json(200, &detail),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/approve")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/approve") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload = match parse_action_payload(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .repository
                .approve_review(review_id, payload.notes.as_deref())
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/reject")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/reject") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload = match parse_action_payload(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .repository
                .reject_review(review_id, payload.notes.as_deref(), false)
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/needs-repair")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/needs-repair") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload = match parse_action_payload(request.body) {
                Ok(payload) => payload,
                Err(error) => {
                    return HttpResponse::json(400, &json!({ "error": error.to_string() }));
                }
            };
            return match self
                .repository
                .reject_review(review_id, payload.notes.as_deref(), true)
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/schema-field")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/schema-field") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let payload: SchemaFieldPayload = match serde_json::from_str(request.body) {
                Ok(value) => value,
                Err(err) => {
                    return HttpResponse::json(400, &json!({ "error": err.to_string() }));
                }
            };
            return match self
                .repository
                .update_schema_field(
                    review_id,
                    payload.schema_index,
                    &payload.field,
                    payload.rule,
                )
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err @ ReviewRepositoryError::InvalidSchemaField(_))
                | Err(err @ ReviewRepositoryError::RequiredSchemaField(_))
                | Err(err @ ReviewRepositoryError::InvalidProductSchemaCandidate)
                | Err(err @ ReviewRepositoryError::UnsupportedArtifact(_, _))
                | Err(err @ ReviewRepositoryError::NotPending(_)) => {
                    HttpResponse::json(400, &json!({ "error": err.to_string() }))
                }
                Err(err) => internal_error(err),
            };
        }

        if request.method == "POST"
            && request.path.starts_with("/api/reviews/")
            && request.path.ends_with("/candidate")
        {
            let Some(review_id) = parse_review_id_with_suffix(request.path, "/candidate") else {
                return HttpResponse::json(400, &json!({ "error": "invalid review id" }));
            };
            let candidate_payload: serde_json::Value = match serde_json::from_str(request.body) {
                Ok(value) => value,
                Err(err) => {
                    return HttpResponse::json(400, &json!({ "error": err.to_string() }));
                }
            };
            return match self
                .repository
                .update_candidate_payload(review_id, candidate_payload)
                .await
            {
                Ok(()) => HttpResponse::json(200, &json!({ "ok": true })),
                Err(err @ ReviewRepositoryError::InvalidSchemaField(_))
                | Err(err @ ReviewRepositoryError::RequiredSchemaField(_))
                | Err(err @ ReviewRepositoryError::InvalidUrlPatternCandidate)
                | Err(err @ ReviewRepositoryError::InvalidProductSchemaCandidate)
                | Err(err @ ReviewRepositoryError::UnsupportedArtifact(_, _))
                | Err(err @ ReviewRepositoryError::NotPending(_)) => {
                    HttpResponse::json(400, &json!({ "error": err.to_string() }))
                }
                Err(err) => internal_error(err),
            };
        }

        HttpResponse::json(404, &json!({ "error": "not found" }))
    }

    fn request_is_authorized(&self, request: &ParsedRequest<'_>) -> bool {
        if !request.path.starts_with("/api/") {
            return true;
        }

        let bearer = self.authorized(&request.headers);
        let mutation_bearer = self.config.auth_token.is_some() && bearer;
        let session = self.has_valid_session(&request.headers);
        match (request.method.as_str(), request.path) {
            ("POST", "/api/session") => mutation_bearer,
            ("POST", "/api/session/logout") => mutation_bearer || session,
            (method, _) if is_mutation_method(method) => mutation_bearer,
            _ => bearer || session,
        }
    }

    fn authorized(&self, headers: &std::collections::HashMap<String, String>) -> bool {
        let Some(expected) = self.config.auth_token.as_deref() else {
            return true;
        };
        headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|actual| constant_time_eq(actual.as_bytes(), expected.as_bytes()))
    }

    fn has_valid_session(&self, headers: &std::collections::HashMap<String, String>) -> bool {
        let Some(session_id) = session_id_from_headers(headers) else {
            return false;
        };
        let now = std::time::Instant::now();
        match self.sessions.get(&session_id) {
            Some(expiry) if *expiry > now => true,
            Some(_) => {
                self.sessions.remove(&session_id);
                false
            }
            None => false,
        }
    }

    fn create_session(&self) -> HttpResponse {
        let now = std::time::Instant::now();
        self.sessions.retain(|_, expiry| *expiry > now);
        let session_id = uuid::Uuid::new_v4();
        self.sessions.insert(session_id, now + REVIEW_SESSION_TTL);
        HttpResponse::json(200, &json!({ "ok": true })).with_header(
            "Set-Cookie",
            session_cookie(&self.config, session_id, REVIEW_SESSION_TTL),
        )
    }

    fn clear_session(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        if let Some(session_id) = session_id_from_headers(&request.headers) {
            self.sessions.remove(&session_id);
        }
        HttpResponse::json(200, &json!({ "ok": true }))
            .with_header("Set-Cookie", expired_session_cookie(&self.config))
    }

    async fn live_review_pages(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Vec<(CrawlerReviewPage, String)>, HttpResponse> {
        let detail = self
            .repository
            .get_review(review_id)
            .await
            .map_err(internal_error)?;
        let mut pages = Vec::with_capacity(detail.pages.len());
        for page in detail.pages {
            let html = self.fetch_live_html(&page).await?;
            pages.push((page, html));
        }
        Ok(pages)
    }

    async fn cached_schema_matrix(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<Option<SchemaMatrix>, ReviewRepositoryError> {
        let detail = self.repository.get_review(review_id).await?;
        let Some(value) = detail.review.validation_summary.get("schema_matrix") else {
            return Ok(None);
        };
        match serde_json::from_value::<SchemaMatrix>(value.clone()) {
            Ok(matrix) => Ok(Some(matrix)),
            Err(err) => {
                warn!(
                    review_id = %review_id,
                    error = ?err,
                    "Cached schema matrix is invalid; refreshing from live pages"
                );
                Ok(None)
            }
        }
    }

    async fn refresh_schema_matrix(
        &self,
        review_id: CrawlerReviewId,
    ) -> Result<SchemaMatrix, HttpResponse> {
        let pages = self.live_review_pages(review_id).await?;
        let evaluated = self
            .repository
            .evaluate_schema_matrix_for_live_pages(review_id, pages)
            .await
            .map_err(internal_error)?;
        self.repository
            .store_schema_matrix_if_current(review_id, &evaluated)
            .await
            .map_err(schema_matrix_store_error)?;
        Ok(evaluated.matrix)
    }

    async fn live_review_page(
        &self,
        review_page_id: CrawlerReviewPageId,
    ) -> Result<Option<(CrawlerReviewPage, String)>, HttpResponse> {
        let Some(page) = self
            .repository
            .get_review_page(review_page_id)
            .await
            .map_err(internal_error)?
        else {
            return Ok(None);
        };
        let html = self.fetch_live_html(&page).await?;
        Ok(Some((page, html)))
    }

    async fn fetch_live_html(&self, page: &CrawlerReviewPage) -> Result<String, HttpResponse> {
        let url = Url::parse(&page.url).map_err(|err| {
            HttpResponse::json(
                400,
                &json!({
                    "error": "invalid live review page url",
                    "url": page.url,
                    "details": err.to_string(),
                }),
            )
        })?;
        self.html_fetcher
            .fetch(&url)
            .await
            .map(|fetched| fetched.html)
            .map_err(|err| live_fetch_error(page, &err))
    }

    async fn fetch_live_url(&self, raw_url: &str) -> Result<String, HttpResponse> {
        let url = Url::parse(raw_url).map_err(|err| {
            HttpResponse::json(
                400,
                &json!({
                    "error": "invalid live preview url",
                    "url": raw_url,
                    "details": err.to_string(),
                }),
            )
        })?;
        self.html_fetcher
            .fetch(&url)
            .await
            .map(|fetched| fetched.html)
            .map_err(|err| live_url_fetch_error(raw_url, &err))
    }
}

fn connection_result(result: Result<std::io::Result<()>, JoinError>) -> std::io::Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            // Peer I/O failure is request-local, not a listener liveness failure.
            warn!(error_kind = ?error.kind(), "Review console connection I/O failed");
            Ok(())
        }
        // Never expose a task's panic payload through runtime errors or logs.
        Err(error) => Err(std::io::Error::other(if error.is_panic() {
            "review connection task panicked"
        } else {
            "review connection task cancelled"
        })),
    }
}

async fn drain_connections(
    connections: &mut JoinSet<std::io::Result<()>>,
    deadline: Instant,
    mut result: std::io::Result<()>,
) -> std::io::Result<()> {
    while !connections.is_empty() {
        let joined = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(deadline) => None,
            joined = connections.join_next() => joined,
        };
        // A ready join can win after the clock passed the deadline, before the timer was polled.
        let expired = joined.is_none() || Instant::now() >= deadline;
        if let Some(joined) = joined {
            result = result.and(connection_result(joined));
        }
        if expired {
            result = result.and(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "review connection drain timed out",
            )));
            connections.abort_all();
            // Do not detach cleanup. Non-cooperative polls/destructors need the caller's watchdog.
            while let Some(joined) = connections.join_next().await {
                result = result.and(connection_result(joined));
            }
            break;
        }
    }
    result
}

async fn write_response<W>(
    writer: &mut W,
    response: &[u8],
    deadline: Duration,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    tokio::time::timeout(deadline, async {
        writer.write_all(response).await?;
        writer.shutdown().await
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "response write timed out"))?
}

struct BoundedRequestHeaders {
    head: String,
    buffered_body: Vec<u8>,
    content_length: usize,
}

async fn read_bounded_request_headers<R>(
    stream: &mut R,
) -> std::io::Result<Option<BoundedRequestHeaders>>
where
    R: AsyncRead + Unpin,
{
    let mut request = Vec::with_capacity(1024);
    let mut byte = [0_u8; 1];
    let header_end = loop {
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        if request.len() >= MAX_REQUEST_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request headers exceed limit",
            ));
        }
        let read = tokio::time::timeout(REQUEST_IO_TIMEOUT, stream.read(&mut byte))
            .await
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "request read timed out")
            })??;
        if read == 0 {
            return if request.is_empty() {
                Ok(None)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "incomplete request headers",
                ))
            };
        }
        request.push(byte[0]);
    };
    let head = String::from_utf8(request[..header_end].to_vec()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request headers are not UTF-8",
        )
    })?;
    let mut content_length = None;
    for line in head.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "transfer encoding is unsupported",
            ));
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "duplicate content length",
                ));
            }
            let length = value.trim().parse::<usize>().map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid content length")
            })?;
            if length > MAX_REQUEST_BODY_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "request body exceeds limit",
                ));
            }
            content_length = Some(length);
        }
    }
    Ok(Some(BoundedRequestHeaders {
        head,
        buffered_body: Vec::new(),
        content_length: content_length.unwrap_or(0),
    }))
}

async fn read_bounded_request_body<R>(
    stream: &mut R,
    mut headers: BoundedRequestHeaders,
) -> std::io::Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut read_buffer = [0_u8; 4096];
    while headers.buffered_body.len() < headers.content_length {
        let remaining = headers.content_length - headers.buffered_body.len();
        let read_limit = read_buffer.len().min(remaining);
        let read = tokio::time::timeout(
            REQUEST_IO_TIMEOUT,
            stream.read(&mut read_buffer[..read_limit]),
        )
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "request read timed out")
        })??;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "incomplete request body",
            ));
        }
        headers
            .buffered_body
            .extend_from_slice(&read_buffer[..read]);
    }
    headers.buffered_body.truncate(headers.content_length);
    let body = String::from_utf8(headers.buffered_body).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "request is not UTF-8")
    })?;
    Ok(headers.head + &body)
}

#[cfg(test)]
async fn read_bounded_request<R>(stream: &mut R) -> std::io::Result<Option<String>>
where
    R: AsyncRead + Unpin,
{
    let Some(headers) = read_bounded_request_headers(stream).await? else {
        return Ok(None);
    };
    read_bounded_request_body(stream, headers).await.map(Some)
}

#[derive(Deserialize, Default)]
struct ActionPayload {
    notes: Option<String>,
}

#[derive(Deserialize)]
struct SchemaFieldPayload {
    schema_index: usize,
    field: String,
    rule: Option<ExtractionRule>,
}

#[derive(Deserialize)]
struct DomainPayload {
    domain: String,
}

fn parse_action_payload(body: &str) -> Result<ActionPayload, serde_json::Error> {
    if body.trim().is_empty() {
        return Ok(ActionPayload::default());
    }
    serde_json::from_str(body)
}

fn session_id_from_headers(
    headers: &std::collections::HashMap<String, String>,
) -> Option<uuid::Uuid> {
    headers
        .get("cookie")?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{REVIEW_SESSION_COOKIE}=")))
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
}

fn session_cookie(config: &ReviewServerConfig, session_id: uuid::Uuid, ttl: Duration) -> String {
    let secure = if config.bind_addr.ip().is_loopback() {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{REVIEW_SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        ttl.as_secs()
    )
}

fn expired_session_cookie(config: &ReviewServerConfig) -> String {
    session_cookie(config, uuid::Uuid::nil(), Duration::ZERO)
}

fn parse_review_id(path: &str) -> Option<CrawlerReviewId> {
    let id = path.strip_prefix("/api/reviews/")?;
    id.parse().ok()
}

fn parse_review_id_with_suffix(path: &str, suffix: &str) -> Option<CrawlerReviewId> {
    let without_suffix = path.strip_suffix(suffix)?;
    parse_review_id(without_suffix)
}

fn parse_listing_source_id_with_suffix(path: &str, suffix: &str) -> Option<ListingSourceId> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/listing-sources/")?;
    id.parse().ok()
}

fn parse_listing_source_domain_id(path: &str) -> Option<(ListingSourceId, CrawlerDomainId)> {
    let rest = path.strip_prefix("/api/listing-sources/")?;
    let (listing_source_id, domain_id) = rest.split_once("/domains/")?;
    Some((listing_source_id.parse().ok()?, domain_id.parse().ok()?))
}

fn parse_page_id_with_suffix(path: &str, suffix: &str) -> Option<CrawlerReviewPageId> {
    let without_suffix = path.strip_suffix(suffix)?;
    let id = without_suffix.strip_prefix("/api/review-pages/")?;
    id.parse().ok()
}

fn domain_configuration_error(error: CrawlerDomainConfigurationError) -> HttpResponse {
    match error {
        CrawlerDomainConfigurationError::ListingSourceNotFound { .. }
        | CrawlerDomainConfigurationError::DomainNotOwnedByListingSource { .. } => {
            HttpResponse::json(404, &json!({ "error": error.to_string() }))
        }
        CrawlerDomainConfigurationError::DomainOwnedByAnotherListingSource { .. } => {
            HttpResponse::json(409, &json!({ "error": error.to_string() }))
        }
        CrawlerDomainConfigurationError::UnsafeDomain { .. }
        | CrawlerDomainConfigurationError::RepeatedWwwPrefix { .. } => {
            HttpResponse::json(400, &json!({ "error": error.to_string() }))
        }
        CrawlerDomainConfigurationError::Database { .. } => internal_error(error),
    }
}

fn schema_matrix_store_error(error: ReviewRepositoryError) -> HttpResponse {
    match error {
        ReviewRepositoryError::CandidateChangedDuringEvaluation => HttpResponse::json(
            409,
            &json!({ "error": "review candidate changed; refresh and evaluate again" }),
        ),
        other => internal_error(other),
    }
}

fn internal_error(error: impl std::fmt::Display + std::fmt::Debug) -> HttpResponse {
    error!(error = ?error, "Review API error");
    HttpResponse::json(500, &json!({ "error": "internal server error" }))
}

fn is_mutation_method(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "PATCH" | "DELETE")
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        difference |= usize::from(*left.get(index).unwrap_or(&0) ^ *right.get(index).unwrap_or(&0));
    }
    difference == 0
}

fn live_fetch_error(page: &CrawlerReviewPage, error: &FetchError) -> HttpResponse {
    warn!(
        review_page_id = %page.review_page_id,
        url = %page.url,
        error = ?error,
        "Failed to fetch live review HTML"
    );
    HttpResponse::json(
        502,
        &json!({
            "error": "failed to fetch live review HTML",
            "url": page.url,
            "details": error.to_string(),
        }),
    )
}

fn live_url_fetch_error(url: &str, error: &FetchError) -> HttpResponse {
    warn!(
        url,
        error = ?error,
        "Failed to fetch live preview HTML"
    );
    HttpResponse::json(
        502,
        &json!({
            "error": "failed to fetch live preview HTML",
            "url": url,
            "details": error.to_string(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::scraper_service::service::FetchedHtml;
    use crate::service::crawler_domain_configuration::{
        CrawlerDomainConfiguration, CrawlerDomainRemoval,
    };

    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Notify, Semaphore, oneshot};

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
    const TEST_TIMEOUT: Duration = Duration::from_secs(3);
    const LIVE_REQUEST: &[u8] = b"GET /api/live-html?url=https%3A%2F%2Fexample.com HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n";

    struct ActiveTask(Arc<AtomicUsize>);

    impl ActiveTask {
        fn new(active: Arc<AtomicUsize>) -> Self {
            active.fetch_add(1, Ordering::SeqCst);
            Self(active)
        }
    }

    impl Drop for ActiveTask {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct GatedHtmlFetcher {
        entered: Semaphore,
        release: Notify,
        active: Arc<AtomicUsize>,
    }

    impl GatedHtmlFetcher {
        fn new() -> Self {
            Self {
                entered: Semaphore::new(0),
                release: Notify::new(),
                active: Arc::new(AtomicUsize::new(0)),
            }
        }

        async fn wait_for_requests(&self, count: u32) -> TestResult {
            tokio::time::timeout(TEST_TIMEOUT, self.entered.acquire_many(count))
                .await??
                .forget();
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl HtmlFetcher for GatedHtmlFetcher {
        async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
            let release = self.release.notified();
            tokio::pin!(release);
            release.as_mut().enable();
            let _active = ActiveTask::new(self.active.clone());
            self.entered.add_permits(1);
            release.await;
            StaticHtmlFetcher.fetch(url).await
        }
    }

    struct RunningReviewServer {
        address: SocketAddr,
        stop: Option<oneshot::Sender<()>>,
        stopped: oneshot::Receiver<()>,
        tasks: JoinSet<std::io::Result<()>>,
    }

    impl RunningReviewServer {
        async fn start(server: ReviewServer, drain: Duration) -> TestResult<Self> {
            let bound = server.bind().await?;
            let address = bound.local_addr()?;
            let (stop, shutdown) = oneshot::channel();
            let (observed, stopped) = oneshot::channel();
            let mut tasks = JoinSet::new();
            tasks.spawn(bound.run_until(
                async move {
                    let _sender_closed = shutdown.await;
                    let _receiver_dropped = observed.send(());
                },
                drain,
            ));
            Ok(Self {
                address,
                stop: Some(stop),
                stopped,
                tasks,
            })
        }

        async fn stop(&mut self) -> TestResult {
            self.stop
                .take()
                .ok_or("server already stopped")?
                .send(())
                .map_err(|_| "server lost shutdown receiver")?;
            tokio::time::timeout(TEST_TIMEOUT, &mut self.stopped).await??;
            Ok(())
        }

        async fn finish(&mut self) -> TestResult<std::io::Result<()>> {
            let joined = tokio::time::timeout(TEST_TIMEOUT, self.tasks.join_next())
                .await?
                .ok_or("server task missing")?;
            assert!(self.tasks.is_empty());
            Ok(joined?)
        }
    }

    async fn socket_response(address: SocketAddr, chunks: &[&[u8]]) -> TestResult<String> {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let mut client = TcpStream::connect(address).await?;
            for chunk in chunks {
                client.write_all(chunk).await?;
                tokio::task::yield_now().await;
            }
            let mut response = String::new();
            client.read_to_string(&mut response).await?;
            Ok::<_, std::io::Error>(response)
        })
        .await?
        .map_err(Into::into)
    }

    async fn assert_socket_closed(client: &mut TcpStream) -> TestResult {
        let mut byte = [0];
        match tokio::time::timeout(TEST_TIMEOUT, client.read(&mut byte)).await? {
            Ok(0) => Ok(()),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) =>
            {
                Ok(())
            }
            result => Err(format!("expected closed socket, got {result:?}").into()),
        }
    }

    struct RejectingDomainAdministration;

    struct StaticHtmlFetcher;

    #[async_trait::async_trait]
    impl HtmlFetcher for StaticHtmlFetcher {
        async fn fetch(&self, url: &Url) -> Result<FetchedHtml, FetchError> {
            Ok(FetchedHtml {
                html: "<script>alert('remote')</script>".to_string(),
                final_url: url.clone(),
            })
        }
    }

    #[async_trait::async_trait]
    impl CrawlerDomainAdministration for RejectingDomainAdministration {
        async fn list_crawler_domains(
            &self,
            listing_source_id: ListingSourceId,
        ) -> Result<Vec<CrawlerDomainConfiguration>, CrawlerDomainConfigurationError> {
            Err(CrawlerDomainConfigurationError::ListingSourceNotFound { listing_source_id })
        }

        async fn register_crawler_domain(
            &self,
            listing_source_id: ListingSourceId,
            _domain: Domain,
        ) -> Result<CrawlerDomainConfiguration, CrawlerDomainConfigurationError> {
            Err(CrawlerDomainConfigurationError::ListingSourceNotFound { listing_source_id })
        }

        async fn remove_crawler_domain(
            &self,
            listing_source_id: ListingSourceId,
            _domain_id: CrawlerDomainId,
        ) -> Result<CrawlerDomainRemoval, CrawlerDomainConfigurationError> {
            Err(CrawlerDomainConfigurationError::ListingSourceNotFound { listing_source_id })
        }
    }

    async fn read_request_from_socket_chunks(chunks: &[&[u8]]) -> std::io::Result<Option<String>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let reader = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            read_bounded_request(&mut stream).await
        });
        let mut client = TcpStream::connect(address).await?;

        for chunk in chunks {
            client.write_all(chunk).await?;
            tokio::task::yield_now().await;
        }
        client.shutdown().await?;

        reader.await.map_err(std::io::Error::other)?
    }

    async fn review_server_for_test(auth_token: Option<&str>) -> Result<ReviewServer, sqlx::Error> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        Ok(ReviewServer::new_with_fetcher(
            CrawlerReviewRepository::new(pool),
            Arc::new(RejectingDomainAdministration),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: auth_token.map(str::to_owned),
            },
            Arc::new(StaticHtmlFetcher),
        ))
    }

    #[tokio::test]
    async fn should_drain_accepted_request_when_stopped() -> TestResult {
        let fetcher = Arc::new(GatedHtmlFetcher::new());
        let mut server = review_server_for_test(Some("review-token")).await?;
        server.html_fetcher = fetcher.clone();
        let mut running = RunningReviewServer::start(server, TEST_TIMEOUT).await?;
        let mut client = TcpStream::connect(running.address).await?;
        client.write_all(LIVE_REQUEST).await?;
        fetcher.wait_for_requests(1).await?;

        running.stop().await?;
        assert!(!running.tasks.is_empty());
        assert!(TcpStream::connect(running.address).await.is_err());
        fetcher.release.notify_waiters();
        let mut response = String::new();
        tokio::time::timeout(TEST_TIMEOUT, client.read_to_string(&mut response)).await??;
        running.finish().await??;

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(response.ends_with("<script>alert('remote')</script>"));
        assert_eq!(fetcher.active.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn should_serve_health_after_peer_reset_while_shutdown_is_pending() -> TestResult {
        let fetcher = Arc::new(GatedHtmlFetcher::new());
        let mut server = review_server_for_test(Some("review-token")).await?;
        server.html_fetcher = fetcher.clone();
        let mut running = RunningReviewServer::start(server, TEST_TIMEOUT).await?;
        let socket = tokio::net::TcpSocket::new_v4()?;
        #[allow(
            deprecated,
            reason = "zero linger forces RST without a blocking linger wait"
        )]
        socket.set_linger(Some(Duration::ZERO))?;
        let mut client = socket.connect(running.address).await?;
        client.write_all(LIVE_REQUEST).await?;
        fetcher.wait_for_requests(1).await?;

        // Reset an accepted request before its response write, rather than closing gracefully.
        drop(client);
        fetcher.release.notify_waiters();
        tokio::time::timeout(TEST_TIMEOUT, async {
            while fetcher.active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await?;

        let response = socket_response(running.address, &[b"GET /health HTTP/1.1\r\n\r\n"]).await?;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.ends_with("{\n  \"ok\": true\n}"));
        assert!(running.tasks.try_join_next().is_none());
        assert!(matches!(
            running.stopped.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        running.stop().await?;
        running.finish().await??;
        Ok(())
    }

    #[tokio::test]
    async fn should_stop_admission_when_all_connection_slots_are_busy() -> TestResult {
        let fetcher = Arc::new(GatedHtmlFetcher::new());
        let mut server = review_server_for_test(Some("review-token")).await?;
        server.html_fetcher = fetcher.clone();
        let mut running = RunningReviewServer::start(server, TEST_TIMEOUT).await?;
        let mut clients = Vec::new();
        for _ in 0..MAX_CONCURRENT_CONNECTIONS {
            let mut client = TcpStream::connect(running.address).await?;
            client.write_all(LIVE_REQUEST).await?;
            clients.push(client);
        }
        fetcher
            .wait_for_requests(MAX_CONCURRENT_CONNECTIONS as u32)
            .await?;
        let mut queued = TcpStream::connect(running.address).await?;
        queued.write_all(LIVE_REQUEST).await?;

        running.stop().await?;
        assert_eq!(
            fetcher.active.load(Ordering::SeqCst),
            MAX_CONCURRENT_CONNECTIONS
        );
        fetcher.release.notify_waiters();
        for client in &mut clients {
            let mut response = String::new();
            tokio::time::timeout(TEST_TIMEOUT, client.read_to_string(&mut response)).await??;
            assert!(response.starts_with("HTTP/1.1 200 OK"));
        }
        running.finish().await??;
        assert_socket_closed(&mut queued).await?;
        assert_eq!(fetcher.active.load(Ordering::SeqCst), 0);
        assert_eq!(fetcher.entered.available_permits(), 0);
        Ok(())
    }

    #[tokio::test]
    async fn should_abort_and_join_connections_when_drain_deadline_expires() -> TestResult {
        let fetcher = Arc::new(GatedHtmlFetcher::new());
        let mut server = review_server_for_test(Some("review-token")).await?;
        server.html_fetcher = fetcher.clone();
        let drain = Duration::from_millis(20);
        let mut running = RunningReviewServer::start(server, drain).await?;
        let mut client = TcpStream::connect(running.address).await?;
        client.write_all(LIVE_REQUEST).await?;
        fetcher.wait_for_requests(1).await?;

        running.stop().await?;
        // New peers cannot reset or extend the original deadline.
        for _ in 0..MAX_CONCURRENT_CONNECTIONS {
            assert!(TcpStream::connect(running.address).await.is_err());
        }
        let error = running.finish().await?.expect_err("drain must time out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "review connection drain timed out");
        // Check before another await: abort without joining would leave this guard alive.
        assert_eq!(fetcher.active.load(Ordering::SeqCst), 0);
        assert_socket_closed(&mut client).await?;
        Ok(())
    }

    #[tokio::test]
    async fn should_stop_when_waiting_for_accept() -> TestResult {
        let server = review_server_for_test(None).await?;
        let mut running = RunningReviewServer::start(server, Duration::ZERO).await?;
        tokio::task::yield_now().await;
        running.stop().await?;
        running.finish().await??;
        assert!(TcpStream::connect(running.address).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn should_reap_completed_connections_before_admitting_more() -> TestResult {
        let server = review_server_for_test(None).await?;
        let mut running = RunningReviewServer::start(server, TEST_TIMEOUT).await?;
        for _ in 0..MAX_CONCURRENT_CONNECTIONS * 2 {
            let response =
                socket_response(running.address, &[b"GET /health HTTP/1.1\r\n\r\n"]).await?;
            assert!(response.starts_with("HTTP/1.1 200 OK"));
        }
        running.stop().await?;
        running.finish().await??;
        Ok(())
    }

    #[rstest::rstest]
    #[case::completed_cleanup(true)]
    #[case::expired_cleanup(false)]
    #[tokio::test]
    async fn should_preserve_accept_failure_while_joining_connections(
        #[case] complete: bool,
    ) -> TestResult {
        let server = review_server_for_test(None).await?;
        let active = Arc::new(AtomicUsize::new(0));
        let guard = ActiveTask::new(active.clone());
        let (release, receiver) = oneshot::channel();
        let mut connections = JoinSet::new();
        connections.spawn(async move {
            let _guard = guard;
            receiver.await.map_err(std::io::Error::other)
        });
        let result = tokio::time::timeout(
            TEST_TIMEOUT,
            server.accept_until(
                || {
                    std::future::ready(Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "synthetic accept failure",
                    )))
                },
                std::future::pending(),
                &mut connections,
            ),
        )
        .await?;
        if complete {
            release.send(()).map_err(|_| "cleanup receiver missing")?;
        }

        let error = drain_connections(
            &mut connections,
            Instant::now() + Duration::from_millis(20),
            result,
        )
        .await
        .expect_err("accept failure must survive cleanup");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "synthetic accept failure");
        assert!(connections.is_empty());
        assert_eq!(active.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[rstest::rstest]
    #[case::broken_pipe(std::io::ErrorKind::BrokenPipe)]
    #[case::connection_reset(std::io::ErrorKind::ConnectionReset)]
    #[case::timed_out(std::io::ErrorKind::TimedOut)]
    #[tokio::test]
    async fn should_keep_peer_io_errors_request_local_during_drain(
        #[case] kind: std::io::ErrorKind,
    ) -> TestResult {
        let mut connections = JoinSet::new();
        connections.spawn(async move { Err(std::io::Error::new(kind, "private request payload")) });

        drain_connections(&mut connections, Instant::now() + TEST_TIMEOUT, Ok(())).await?;
        assert!(connections.is_empty());
        Ok(())
    }

    #[rstest::rstest]
    #[case::unexpected_cancellation(false)]
    #[case::panic(true)]
    #[tokio::test]
    async fn should_preserve_task_failure_while_joining_other_connections(
        #[case] panic_task: bool,
    ) -> TestResult {
        let server = review_server_for_test(None).await?;
        let active = Arc::new(AtomicUsize::new(0));
        let guard = ActiveTask::new(active.clone());
        let mut connections = JoinSet::new();
        connections.spawn(async move {
            let _guard = guard;
            std::future::pending::<std::io::Result<()>>().await
        });
        let task = connections.spawn(async move {
            if panic_task {
                std::panic::resume_unwind(Box::new("private task payload"));
            }
            std::future::pending::<std::io::Result<()>>().await
        });
        if !panic_task {
            task.abort();
        }
        let result = tokio::time::timeout(
            TEST_TIMEOUT,
            server.accept_until(
                std::future::pending,
                std::future::pending(),
                &mut connections,
            ),
        )
        .await?;

        let error = drain_connections(
            &mut connections,
            Instant::now() + Duration::from_millis(20),
            result,
        )
        .await
        .expect_err("task failure must survive cleanup");
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        if panic_task {
            assert_eq!(error.to_string(), "review connection task panicked");
        } else {
            assert_eq!(error.to_string(), "review connection task cancelled");
        }
        assert!(connections.is_empty());
        assert_eq!(active.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn should_reject_ready_completion_after_absolute_deadline() {
        let mut connections = JoinSet::new();
        connections.spawn(async { Ok(()) });
        tokio::task::yield_now().await;
        let deadline = Instant::now() + Duration::from_millis(10);
        tokio::time::advance(Duration::from_millis(11)).await;

        let error = drain_connections(&mut connections, deadline, Ok(()))
            .await
            .expect_err("late completion must not report success");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(connections.is_empty());
    }

    #[tokio::test]
    async fn should_reject_completion_when_task_poll_runs_past_drain_deadline() {
        let mut connections = JoinSet::new();
        connections.spawn(async {
            // Bounded non-yielding poll: completion can be ready before the timer driver runs.
            std::thread::sleep(Duration::from_millis(30));
            Ok(())
        });
        let error = drain_connections(
            &mut connections,
            Instant::now() + Duration::from_millis(5),
            Ok(()),
        )
        .await
        .expect_err("late completion must not report success");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(connections.is_empty());
    }

    #[tokio::test]
    async fn should_keep_auth_cookie_body_and_response_contracts_over_real_sockets() -> TestResult {
        let server = review_server_for_test(Some("review-token")).await?;
        let mut running = RunningReviewServer::start(server, TEST_TIMEOUT).await?;
        for request in [
            "GET /api/health HTTP/1.1\r\n\r\n",
            "GET /api/health HTTP/1.1\r\nAuthorization: Bearer wrong-token\r\n\r\n",
            // Missing body must not delay auth rejection until the 10s read timeout.
            "POST /api/session HTTP/1.1\r\nContent-Length: 64\r\n\r\n",
        ] {
            let response = socket_response(running.address, &[request.as_bytes()]).await?;
            assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));
        }
        let login = socket_response(running.address, &[
            b"POST /api/session HTTP/1.1\r\nAuthorization: Bearer review-token\r\nContent-Length: 2\r\n\r\n{",
            b"}",
        ]).await?;
        assert!(login.starts_with("HTTP/1.1 200 OK"));
        assert!(login.contains("HttpOnly"));
        assert!(login.contains("SameSite=Strict"));
        assert!(!login.contains("Secure"));
        assert!(!login.contains("review-token"));
        let cookie = login
            .lines()
            .find_map(|line| line.strip_prefix("Set-Cookie: "))
            .and_then(|value| value.split(';').next())
            .ok_or("session cookie missing")?;
        for (request, status) in [
            (
                format!("GET /api/health HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"),
                200,
            ),
            (
                format!("POST /api/session HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"),
                401,
            ),
            (
                format!("POST /api/reviews/invalid/approve HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"),
                401,
            ),
            (
                format!("POST /api/session/logout HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"),
                200,
            ),
            (
                format!("GET /api/health HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"),
                401,
            ),
        ] {
            let response = socket_response(running.address, &[request.as_bytes()]).await?;
            assert!(response.starts_with(&format!("HTTP/1.1 {status}")));
        }
        for request in [
            format!(
                "GET /health HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                MAX_REQUEST_BODY_BYTES + 1
            ),
            "GET /health HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n".to_string(),
            "GET /health HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n".to_string(),
        ] {
            let response = socket_response(running.address, &[request.as_bytes()]).await?;
            assert!(response.starts_with("HTTP/1.1 400 Bad Request"));
        }
        let request = format!(
            "GET /health HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            MAX_REQUEST_BODY_BYTES,
            "a".repeat(MAX_REQUEST_BODY_BYTES),
        );
        let response = socket_response(running.address, &[request.as_bytes()]).await?;
        let (headers, body) = response
            .split_once("\r\n\r\n")
            .ok_or("response framing missing")?;
        assert!(headers.starts_with("HTTP/1.1 200 OK"));
        assert!(headers.contains("Content-Type: application/json; charset=utf-8"));
        assert!(headers.contains(&format!("Content-Length: {}", body.len())));
        assert!(headers.contains("Connection: close"));
        assert!(headers.contains("X-Content-Type-Options: nosniff"));
        assert!(headers.contains("Content-Security-Policy:"));
        assert_eq!(body, "{\n  \"ok\": true\n}");
        running.stop().await?;
        running.finish().await??;
        Ok(())
    }

    #[tokio::test]
    async fn should_require_mutation_token_on_loopback_without_configured_token() -> TestResult {
        let server = review_server_for_test(None).await?;
        let mut running = RunningReviewServer::start(server, TEST_TIMEOUT).await?;
        let response =
            socket_response(running.address, &[b"GET /api/health HTTP/1.1\r\n\r\n"]).await?;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        for request in [
            "POST /api/session HTTP/1.1\r\n\r\n",
            "POST /api/session HTTP/1.1\r\nAuthorization: Bearer arbitrary\r\n\r\n",
        ] {
            let response = socket_response(running.address, &[request.as_bytes()]).await?;
            assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));
        }
        running.stop().await?;
        running.finish().await??;
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_run_until_wrapper_and_validate_non_loopback_bind() -> TestResult {
        let server = review_server_for_test(None).await?;
        tokio::time::timeout(TEST_TIMEOUT, server.run_until(async {}, Duration::ZERO)).await??;
        let mut server = review_server_for_test(None).await?;
        server.config.bind_addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let result = server.run_until(async {}, Duration::ZERO).await;
        assert_eq!(
            result.expect_err("non-loopback bind requires token").kind(),
            std::io::ErrorKind::InvalidInput
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_total_request_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let server = review_server_for_test(None).await?;
        let (_client, mut stream) = tokio::io::duplex(1024);

        let response = server
            .response_for_request(&mut stream, Duration::from_millis(10))
            .await;

        let response = match response {
            Some(response) => response,
            None => return Err("request deadline returned no response".into()),
        };
        assert!(response.to_string().starts_with("HTTP/1.1 400 Bad Request"));
        Ok(())
    }

    #[tokio::test]
    async fn should_apply_response_write_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let (mut writer, _reader) = tokio::io::duplex(1);

        let error = match write_response(&mut writer, b"response", Duration::from_millis(10)).await
        {
            Ok(()) => return Err("response write should time out".into()),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(error.to_string(), "response write timed out");
        Ok(())
    }

    #[tokio::test]
    async fn should_accept_fragmented_content_length_body() -> Result<(), Box<dyn std::error::Error>>
    {
        let request = read_request_from_socket_chunks(&[
            b"POST /api/health HTTP/1.1\r\nContent-Length: 15\r\n\r\n{\"key\":",
            b"\"value\"}",
        ])
        .await?;

        assert_eq!(
            request.as_deref(),
            Some("POST /api/health HTTP/1.1\r\nContent-Length: 15\r\n\r\n{\"key\":\"value\"}")
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_authenticate_api_headers_before_waiting_for_declared_body()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = review_server_for_test(Some("review-token")).await?;
        let (mut client, mut stream) = tokio::io::duplex(1024);
        client
            .write_all(b"POST /api/health HTTP/1.1\r\nContent-Length: 64\r\n\r\n")
            .await?;

        let response = tokio::time::timeout(
            Duration::from_millis(100),
            server.response_for_request(&mut stream, Duration::from_secs(1)),
        )
        .await?
        .ok_or("unauthorized request returned no response")?;

        assert!(
            response
                .to_string()
                .starts_with("HTTP/1.1 401 Unauthorized")
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_header_overshoot() -> Result<(), Box<dyn std::error::Error>> {
        let prefix = "GET / HTTP/1.1\r\nX-Long: ";
        let request = format!(
            "{prefix}{}\r\n\r\n",
            "a".repeat(MAX_REQUEST_HEADER_BYTES - prefix.len() + 1)
        );

        let error = read_request_from_socket_chunks(&[request.as_bytes()])
            .await
            .expect_err("oversized request headers should be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "request headers exceed limit");
        Ok(())
    }

    #[tokio::test]
    async fn should_serve_raw_live_html_as_plain_text() -> Result<(), Box<dyn std::error::Error>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        let server = ReviewServer::new_with_fetcher(
            CrawlerReviewRepository::new(pool),
            Arc::new(RejectingDomainAdministration),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: None,
            },
            Arc::new(StaticHtmlFetcher),
        );

        let response = server
            .route("GET /api/live-html?url=https%3A%2F%2Fexample.com HTTP/1.1\r\n\r\n")
            .await
            .to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Content-Type: text/plain; charset=utf-8"));
        assert!(response.ends_with("<script>alert('remote')</script>"));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_request_body_larger_than_limit() -> Result<(), Box<dyn std::error::Error>>
    {
        let request = format!(
            "POST /api/health HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_REQUEST_BODY_BYTES + 1
        );
        let error = read_request_from_socket_chunks(&[request.as_bytes()])
            .await
            .expect_err("oversized request body should be rejected");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "request body exceeds limit");
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_transfer_encoding_and_duplicate_content_length()
    -> Result<(), Box<dyn std::error::Error>> {
        for (request, expected_error) in [
            (
                "POST /api/health HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n",
                "transfer encoding is unsupported",
            ),
            (
                "POST /api/health HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
                "duplicate content length",
            ),
        ] {
            let error = read_request_from_socket_chunks(&[request.as_bytes()])
                .await
                .expect_err("ambiguous request framing should be rejected");

            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), expected_error);
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_allow_cookie_session_for_protected_navigation_and_invalidate_it_on_logout()
    -> Result<(), Box<dyn std::error::Error>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/crawler")?;
        let server = ReviewServer::new_with_fetcher(
            CrawlerReviewRepository::new(pool),
            Arc::new(RejectingDomainAdministration),
            ReviewServerConfig {
                bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
                auth_token: Some("review-token".to_string()),
            },
            Arc::new(StaticHtmlFetcher),
        );
        let login = server
            .route("POST /api/session HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n")
            .await
            .to_string();
        let cookie = login
            .lines()
            .find_map(|line| line.strip_prefix("Set-Cookie: "))
            .ok_or("session response did not set a cookie")?
            .split(';')
            .next()
            .ok_or("session cookie was empty")?;

        assert!(login.contains("HttpOnly"));
        assert!(login.contains("SameSite=Strict"));
        assert!(!login.contains("review-token"));

        let reviews_raw_request = format!("GET /api/reviews HTTP/1.1\r\nCookie: {cookie}\r\n\r\n");
        let reviews_request =
            parse_request(&reviews_raw_request).ok_or("reviews request should parse")?;
        assert!(server.request_is_authorized(&reviews_request));

        let navigation = server
            .route(&format!(
                "GET /api/live-inspect?url=https%3A%2F%2Fexample.com HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"
            ))
            .await
            .to_string();
        assert!(navigation.starts_with("HTTP/1.1 200 OK"));
        assert!(navigation.contains("data-crawler-review-disabled"));

        for request in [
            format!("POST /api/session HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"),
            format!(
                "POST /api/reviews/{}/approve HTTP/1.1\r\nCookie: {cookie}\r\n\r\n",
                uuid::Uuid::new_v4()
            ),
            format!(
                "POST /api/listing-sources/{}/domains HTTP/1.1\r\nCookie: {cookie}\r\n\r\n",
                uuid::Uuid::new_v4()
            ),
        ] {
            let response = server.route(&request).await.to_string();
            assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));
        }

        let bearer_mutation = server
            .route("POST /api/session HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n")
            .await
            .to_string();
        assert!(bearer_mutation.starts_with("HTTP/1.1 200 OK"));

        let logout = server
            .route(&format!(
                "POST /api/session/logout HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"
            ))
            .await
            .to_string();
        assert!(logout.starts_with("HTTP/1.1 200 OK"));
        assert!(logout.contains("Max-Age=0"));

        let expired = server
            .route(&format!(
                "GET /api/health HTTP/1.1\r\nCookie: {cookie}\r\n\r\n"
            ))
            .await
            .to_string();
        assert!(expired.starts_with("HTTP/1.1 401 Unauthorized"));
        Ok(())
    }

    #[test]
    fn should_require_authentication_for_non_loopback_review_bind() {
        let result = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: None,
        }
        .validate();

        assert!(matches!(
            result,
            Err(ReviewServerConfigError::AuthenticationRequiredForNonLoopback)
        ));
    }

    #[test]
    fn should_allow_non_loopback_review_bind_with_authentication_token() {
        let result = ReviewServerConfig {
            bind_addr: "0.0.0.0:7878".parse().unwrap(),
            auth_token: Some("review-token".to_string()),
        }
        .validate();

        assert!(result.is_ok());
    }

    #[test]
    fn should_allow_loopback_review_bind_without_authentication_token() {
        let result = ReviewServerConfig {
            bind_addr: "127.0.0.1:7878".parse().unwrap(),
            auth_token: None,
        }
        .validate();

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_expose_minimal_unauthenticated_liveness_health()
    -> Result<(), Box<dyn std::error::Error>> {
        let server = review_server_for_test(Some("review-token")).await?;

        let response = server
            .route("GET /health HTTP/1.1\r\n\r\n")
            .await
            .to_string();

        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.ends_with("{\n  \"ok\": true\n}"));
        Ok(())
    }

    #[tokio::test]
    async fn should_require_valid_token_for_configured_api_access_and_all_mutations()
    -> Result<(), Box<dyn std::error::Error>> {
        let unauthenticated_server = review_server_for_test(None).await?;
        let unauthenticated_read = unauthenticated_server
            .route("GET /api/health HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        let unauthenticated_mutation = unauthenticated_server
            .route("POST /api/health HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        assert!(unauthenticated_read.starts_with("HTTP/1.1 200"));
        assert!(unauthenticated_mutation.starts_with("HTTP/1.1 401"));

        let authenticated_server = review_server_for_test(Some("review-token")).await?;
        let missing_token = authenticated_server
            .route("GET /api/health HTTP/1.1\r\n\r\n")
            .await
            .to_string();
        let invalid_token = authenticated_server
            .route("GET /api/health HTTP/1.1\r\nAuthorization: Bearer wrong-token\r\n\r\n")
            .await
            .to_string();
        let valid_token = authenticated_server
            .route("GET /api/health HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n")
            .await
            .to_string();
        let valid_mutation_token = authenticated_server
            .route("POST /api/health HTTP/1.1\r\nAuthorization: Bearer review-token\r\n\r\n")
            .await
            .to_string();

        assert!(missing_token.starts_with("HTTP/1.1 401"));
        assert!(invalid_token.starts_with("HTTP/1.1 401"));
        assert!(valid_token.starts_with("HTTP/1.1 200"));
        assert!(valid_mutation_token.starts_with("HTTP/1.1 404"));
        Ok(())
    }

    #[test]
    fn should_compare_authentication_tokens_without_early_value_match() {
        assert!(constant_time_eq(b"token", b"token"));
        assert!(!constant_time_eq(b"token", b"taken"));
        assert!(!constant_time_eq(b"token", b"tokens"));
    }
}

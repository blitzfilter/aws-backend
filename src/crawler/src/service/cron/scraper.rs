use super::job::CrawlerCronJob;
use crate::network::policy::{NetworkErrorKind, durable_retry_cooldown_for};
use crate::scraper::candidate_service::{ScraperCandidate, ScraperCandidateService};
use crate::scraper::raw_input::{crawler_provenance, crawler_verified_removal_input};
use crate::scraper::scraper_service::{ScrapedProduct, ScraperError, ScraperService};
use crate::service::raw_capture::{
    ProductListingRawCaptureItem, ProductListingRawCaptureOutcome, ProductListingRawCaptureService,
};
use crate::spider::advisory_lock::{ListingSourceLock, LocalLockManager, UrlLock};
use crate::spider::classification::url_metadata::{CrawlerDisposition, CrawlerUrlWriteOutcome};
use futures::FutureExt;
use listing_source_core::ListingSourceId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinSet;
use tracing::{Instrument, debug, error, info, warn};

/// Context for scraping a domain's candidates.
struct ScrapeDomainContext {
    scraper: Arc<dyn ScraperService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    lock_manager: Arc<LocalLockManager>,
    command_tx: mpsc::Sender<QueuedRawCapture>,
    budget_exhausted_listing_sources: Arc<Mutex<HashSet<ListingSourceId>>>,
    schema_pending_listing_sources: Arc<Mutex<HashSet<ListingSourceId>>>,
    failure: OperationalFailureSignal,
}

/// Metadata applied only after durable raw capture succeeds.
struct CandidateMeta {
    listing_source_id: listing_source_core::ListingSourceId,
    url: url::Url,
    hash: String,
    schema_fingerprint: String,
    raw_input_sha256: Vec<u8>,
    disposition: CrawlerDisposition,
    expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
}

struct RawCaptureRequest {
    item: ProductListingRawCaptureItem,
    on_success: RawCaptureSuccessAction,
    on_failure: RawCaptureFailureAction,
}

enum RawCaptureSuccessAction {
    MarkScraped(CandidateMeta),
    MarkRemoved {
        listing_source_id: ListingSourceId,
        url: url::Url,
        raw_input_sha256: Vec<u8>,
        expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
    },
}

enum RawCaptureFailureAction {
    None,
    MarkScraperFailure {
        listing_source_id: ListingSourceId,
        url: url::Url,
        expected_last_captured_raw_input_sha256: Option<Vec<u8>>,
    },
}

impl From<(ProductListingRawCaptureItem, CandidateMeta)> for RawCaptureRequest {
    fn from((item, meta): (ProductListingRawCaptureItem, CandidateMeta)) -> Self {
        Self {
            item,
            on_success: RawCaptureSuccessAction::MarkScraped(meta),
            on_failure: RawCaptureFailureAction::None,
        }
    }
}

struct QueuedRawCapture {
    request: RawCaptureRequest,
    enqueued_at: tokio::time::Instant,
}

struct ScrapeCandidateOutcome {
    capture: Option<RawCaptureRequest>,
    errored: bool,
    skipped: bool,
    operational_failed: bool,
}

#[derive(Default)]
struct ScrapeDomainOutcome {
    accepted: usize,
    failed: usize,
    operational_failed: usize,
    skipped: usize,
    admission_stopped: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct RawCaptureDrainSummary {
    accepted: usize,
    durable: usize,
    completed: usize,
    stale: usize,
    discarded_missing_source: usize,
    capture_failed: usize,
    local_mark_failed: usize,
    failure_mark_failed: usize,
    result_mismatches: usize,
}

impl RawCaptureDrainSummary {
    fn merge(&mut self, batch: Self) {
        self.accepted += batch.accepted;
        self.durable += batch.durable;
        self.completed += batch.completed;
        self.stale += batch.stale;
        self.discarded_missing_source += batch.discarded_missing_source;
        self.capture_failed += batch.capture_failed;
        self.local_mark_failed += batch.local_mark_failed;
        self.failure_mark_failed += batch.failure_mark_failed;
        self.result_mismatches += batch.result_mismatches;
    }

    fn unfinished(&self) -> usize {
        self.capture_failed + self.local_mark_failed
    }

    fn is_complete(&self) -> bool {
        self.unfinished() == 0 && self.failure_mark_failed == 0 && self.result_mismatches == 0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("crawler raw capture drain incomplete")]
struct RawCaptureDrainError {
    summary: RawCaptureDrainSummary,
}

/// Counts are confirmed outcomes only. A failed task leaves its unreported work unknown.
#[must_use]
#[derive(Debug, Default)]
pub(super) struct ScraperPassOutcome {
    total: usize,
    accepted: usize,
    captures: RawCaptureDrainSummary,
    failed: usize,
    operational_failed: usize,
    operational_failure_notified: bool,
    skipped: usize,
    admission_stopped: bool,
    scope_refresh_failed: bool,
    candidate_lookup_failed: bool,
    worker_failed: bool,
    collector_failed: bool,
}

impl ScraperPassOutcome {
    /// A stopped pass can be complete: interrupted, unaccepted URLs remain due for retry.
    pub(super) fn admission_stopped(&self) -> bool {
        self.admission_stopped
    }

    /// Confirms admitted capture custody and task success, not exhaustion of due URLs.
    pub(super) fn is_complete(&self) -> bool {
        self.failed == 0 && !self.has_operational_failure()
    }

    /// Site/provider/budget/review failures remain due for another pass after their
    /// normal metadata writes. Custody, metadata, and unknown-task failures stop the job.
    pub(super) fn has_operational_failure(&self) -> bool {
        self.operational_failed != 0
            || self.operational_failure_notified
            || !self.captures.is_complete()
            || self.scope_refresh_failed
            || self.candidate_lookup_failed
            || self.worker_failed
            || self.collector_failed
            || self.accepted != self.captures.accepted
    }
}

#[derive(Clone)]
struct OperationalFailureSignal {
    failed: watch::Sender<bool>,
    shared_stop: Option<watch::Sender<bool>>,
}

impl OperationalFailureSignal {
    fn new(shared_stop: Option<watch::Sender<bool>>) -> Self {
        let (failed, _) = watch::channel(false);
        Self {
            failed,
            shared_stop,
        }
    }

    fn notify(&self) {
        self.failed.send_replace(true);
        if let Some(stop) = &self.shared_stop {
            stop.send_replace(true);
        }
    }

    fn is_notified(&self) -> bool {
        *self.failed.borrow()
    }

    async fn notified(&self) {
        wait_for_scraper_stop(&mut self.failed.subscribe()).await;
    }
}

impl Default for OperationalFailureSignal {
    fn default() -> Self {
        Self::new(None)
    }
}

// Catch_unwind polls a future but does not guard every destructor. Keep this
// outside those boundaries so an unwinding destructor still starts sibling drain.
struct NotifyFailureOnUnwind(OperationalFailureSignal);

impl Drop for NotifyFailureOnUnwind {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.notify();
        }
    }
}

struct ScheduledScrapeDomainOutcome {
    domain: String,
    outcome: ScrapeDomainOutcome,
}

/// Captures a batch of raw observations, then updates crawler-local metadata only for durable
/// capture outcomes. Worker normalization owns canonical ProductListing mutation.
#[tracing::instrument(
    name = "crawler_flush_raw_capture_batch",
    skip(raw_capture, scraper_candidates, batch, failure),
    fields(batch_size = batch.len())
)]
async fn flush_batch_with_failure(
    raw_capture: &Arc<dyn ProductListingRawCaptureService>,
    scraper_candidates: &Arc<dyn ScraperCandidateService>,
    batch: Vec<QueuedRawCapture>,
    queue_depth: usize,
    failure: &OperationalFailureSignal,
) -> RawCaptureDrainSummary {
    let batch_size = batch.len();
    let oldest_item_age_ms = batch
        .iter()
        .map(|queued| queued.enqueued_at.elapsed().as_millis())
        .max()
        .unwrap_or(0);
    let observations = batch
        .iter()
        .map(|queued| queued.request.item.clone())
        .collect();

    let capture_started_at = tokio::time::Instant::now();
    let mut outcomes = raw_capture.capture(observations).await;
    let capture_latency_ms = capture_started_at.elapsed().as_millis();
    let expected = batch.len();
    let actual = outcomes.len();

    if actual != expected {
        failure.notify();
        warn!(
            expected,
            actual,
            "Raw ProductListing capture returned an incomplete result; unmatched URLs will be retried"
        );
    }

    outcomes.truncate(expected);
    if outcomes.len() < expected {
        outcomes.resize(expected, ProductListingRawCaptureOutcome::RetryableFailure);
    }

    let persisted_count = outcomes
        .iter()
        .filter(|outcome| **outcome == ProductListingRawCaptureOutcome::Persisted)
        .count();
    let discarded_missing_source_count = outcomes
        .iter()
        .filter(|outcome| **outcome == ProductListingRawCaptureOutcome::DiscardedMissingSource)
        .count();
    let persistence_failure_count = outcomes
        .iter()
        .filter(|outcome| **outcome == ProductListingRawCaptureOutcome::RetryableFailure)
        .count();
    if persistence_failure_count != 0 {
        failure.notify();
    }
    let mut mark_as_scraped_count = 0;
    let mut stale_completion_count = 0;
    let mut mark_as_scraped_failure_count = 0;
    let mut failure_mark_failed = 0;

    for (queued, outcome) in batch.into_iter().zip(outcomes) {
        match (
            outcome,
            queued.request.on_success,
            queued.request.on_failure,
        ) {
            (
                ProductListingRawCaptureOutcome::Persisted,
                RawCaptureSuccessAction::MarkScraped(meta),
                _,
            ) => {
                match scraper_candidates
                    .mark_as_scraped(
                        &meta.listing_source_id,
                        &meta.url,
                        &meta.hash,
                        &meta.schema_fingerprint,
                        &meta.raw_input_sha256,
                        meta.disposition,
                        meta.expected_last_captured_raw_input_sha256.as_deref(),
                    )
                    .await
                {
                    Ok(CrawlerUrlWriteOutcome::Applied) => {
                        mark_as_scraped_count += 1;
                        if meta.disposition != CrawlerDisposition::Active {
                            info!(
                                metric = "crawler_disposition_transition",
                                crawler_disposition_transitions = 1_u64,
                                listing_source_id = %meta.listing_source_id,
                                crawler_disposition = meta.disposition.as_str(),
                                "crawler URL entered dormant disposition after durable raw capture"
                            );
                        }
                    }
                    Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                        stale_completion_count += 1;
                        debug!(
                            listing_source_id = %meta.listing_source_id,
                            url = %meta.url,
                            crawler_url_write_outcome = "stale_noop",
                            "Skipped stale crawler scrape completion after durable raw capture"
                        );
                    }
                    Err(_) => {
                        mark_as_scraped_failure_count += 1;
                        failure.notify();
                        warn!(listing_source_id = %meta.listing_source_id, error_kind = "local_mark_failed", "Failed to mark product as scraped after raw capture");
                    }
                }
            }
            (
                ProductListingRawCaptureOutcome::Persisted,
                RawCaptureSuccessAction::MarkRemoved {
                    listing_source_id,
                    url,
                    raw_input_sha256,
                    expected_last_captured_raw_input_sha256,
                },
                _,
            ) => match scraper_candidates
                .mark_removed(
                    &listing_source_id,
                    &url,
                    &raw_input_sha256,
                    expected_last_captured_raw_input_sha256.as_deref(),
                )
                .await
            {
                Ok(CrawlerUrlWriteOutcome::Applied) => {
                    mark_as_scraped_count += 1;
                }
                Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                    stale_completion_count += 1;
                    debug!(
                        listing_source_id = %listing_source_id,
                        url = %url,
                        crawler_url_write_outcome = "stale_noop",
                        "Skipped stale crawler removal completion after durable raw capture"
                    );
                }
                Err(_) => {
                    mark_as_scraped_failure_count += 1;
                    failure.notify();
                    warn!(listing_source_id = %listing_source_id, error_kind = "local_mark_failed", "Raw removal capture committed but crawler scrape metadata update failed");
                }
            },
            (
                ProductListingRawCaptureOutcome::RetryableFailure,
                _,
                RawCaptureFailureAction::MarkScraperFailure {
                    listing_source_id,
                    url,
                    expected_last_captured_raw_input_sha256,
                },
            ) => match scraper_candidates
                .mark_scraper_failure(
                    &listing_source_id,
                    &url,
                    "RawCaptureFailed",
                    "verified removal raw capture did not commit",
                    expected_last_captured_raw_input_sha256.as_deref(),
                )
                .await
            {
                Ok(CrawlerUrlWriteOutcome::Applied) => {}
                Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                    debug!(
                        listing_source_id = %listing_source_id,
                        url = %url,
                        crawler_url_write_outcome = "stale_noop",
                        "Skipped stale crawler raw-capture failure metadata"
                    );
                }
                Err(_) => {
                    failure_mark_failed += 1;
                    failure.notify();
                    warn!(
                        error_kind = "failure_mark_failed",
                        listing_source_id = %listing_source_id,
                        "Failed to persist raw-capture failure metadata"
                    );
                }
            },
            (
                ProductListingRawCaptureOutcome::RetryableFailure,
                _,
                RawCaptureFailureAction::None,
            ) => {}
            (ProductListingRawCaptureOutcome::DiscardedMissingSource, _, _) => {
                debug!(
                    outcome = "discarded_missing_source",
                    "Discarded crawler raw-capture completion without marking it persisted"
                );
            }
        }
    }

    info!(
        event = "crawler.raw_capture.batch",
        batch_size,
        queue_depth,
        oldest_item_age_ms,
        capture_latency_ms,
        persisted_count,
        discarded_missing_source_count,
        persistence_failure_count,
        mark_as_scraped_count,
        stale_completion_count,
        mark_as_scraped_failure_count,
        failure_mark_failed,
        "Crawler raw capture batch finished"
    );

    RawCaptureDrainSummary {
        accepted: batch_size,
        durable: persisted_count,
        completed: mark_as_scraped_count,
        stale: stale_completion_count,
        discarded_missing_source: discarded_missing_source_count,
        capture_failed: persistence_failure_count,
        local_mark_failed: mark_as_scraped_failure_count,
        failure_mark_failed,
        result_mismatches: usize::from(actual != expected),
    }
}

#[cfg(test)]
async fn flush_batch(
    raw_capture: &Arc<dyn ProductListingRawCaptureService>,
    scraper_candidates: &Arc<dyn ScraperCandidateService>,
    batch: Vec<QueuedRawCapture>,
    queue_depth: usize,
) -> RawCaptureDrainSummary {
    flush_batch_with_failure(
        raw_capture,
        scraper_candidates,
        batch,
        queue_depth,
        &OperationalFailureSignal::default(),
    )
    .await
}

fn scraper_stop_requested(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow() || stop.has_changed().is_err()
}

async fn wait_for_scraper_stop(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

/// None means stopped before acceptance; no completion metadata may be written for that input.
async fn enqueue_raw_capture(
    command_tx: &mpsc::Sender<QueuedRawCapture>,
    request: impl Into<RawCaptureRequest>,
    stop: &mut watch::Receiver<bool>,
) -> Result<Option<Duration>, mpsc::error::SendError<()>> {
    let queued = QueuedRawCapture {
        request: request.into(),
        enqueued_at: tokio::time::Instant::now(),
    };
    let wait_started_at = tokio::time::Instant::now();
    let permit = tokio::select! {
        biased;
        _ = wait_for_scraper_stop(stop) => return Ok(None),
        permit = command_tx.reserve() => permit?,
    };

    // Recheck after capacity becomes available. Keep the read guard through the synchronous
    // send so a concurrent true signal cannot overtake this final admission check.
    let stopping = stop.borrow();
    if *stopping || stop.has_changed().is_err() {
        return Ok(None);
    }
    permit.send(queued);

    Ok(Some(wait_started_at.elapsed()))
}

#[tracing::instrument(
    name = "crawler_scrape_candidate",
    skip(candidate),
    fields(
        listing_source_id = %candidate.listing_source_id,
        url = %candidate.url
    )
)]
fn handle_verified_removal(candidate: &ScraperCandidate) -> ScrapeCandidateOutcome {
    let input = match crawler_verified_removal_input(&candidate.url) {
        Ok(input) => input,
        Err(error) => {
            warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to build verified-removal raw input");
            return ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                operational_failed: true,
            };
        }
    };
    let raw_input_sha256 = match input.hash() {
        Ok(hash) => hash.as_bytes().to_vec(),
        Err(error) => {
            warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to hash verified-removal raw input");
            return ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                operational_failed: true,
            };
        }
    };
    let provenance = match crawler_provenance(None, None) {
        Ok(provenance) => provenance,
        Err(error) => {
            warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to build verified-removal provenance");
            return ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                operational_failed: true,
            };
        }
    };

    ScrapeCandidateOutcome {
        capture: Some(RawCaptureRequest {
            item: ProductListingRawCaptureItem::crawler(
                candidate.listing_source_id,
                &candidate.url,
                input,
                provenance,
            ),
            on_success: RawCaptureSuccessAction::MarkRemoved {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                raw_input_sha256,
                expected_last_captured_raw_input_sha256: candidate
                    .last_captured_raw_input_sha256
                    .clone(),
            },
            on_failure: RawCaptureFailureAction::MarkScraperFailure {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                expected_last_captured_raw_input_sha256: candidate
                    .last_captured_raw_input_sha256
                    .clone(),
            },
        }),
        errored: false,
        skipped: false,
        operational_failed: false,
    }
}

async fn scrape_candidate(
    candidate: ScraperCandidate,
    ctx: &ScrapeDomainContext,
    stop: &mut watch::Receiver<bool>,
) -> Option<ScrapeCandidateOutcome> {
    if scraper_stop_requested(stop) || ctx.failure.is_notified() {
        return None;
    }
    // Skip URLs from listing_sources with already-exhausted budgets
    {
        let exhausted = ctx.budget_exhausted_listing_sources.lock().await;
        if exhausted.contains(&candidate.listing_source_id) {
            debug!("Skipping URL — ListingSource LLM budget already exhausted in this batch");
            return Some(ScrapeCandidateOutcome {
                capture: None,
                errored: false,
                skipped: true,
                operational_failed: false,
            });
        }
    }

    {
        let pending = ctx.schema_pending_listing_sources.lock().await;
        if pending.contains(&candidate.listing_source_id) {
            debug!("Skipping URL because ListingSource has pending schema review in this batch");
            return Some(ScrapeCandidateOutcome {
                capture: None,
                errored: false,
                skipped: true,
                operational_failed: false,
            });
        }
    }

    let Some(_lock) = UrlLock::try_acquire(&ctx.lock_manager, &candidate.url) else {
        warn!("Skipping URL — lock held by another worker");
        return Some(ScrapeCandidateOutcome {
            capture: None,
            errored: false,
            skipped: true,
            operational_failed: false,
        });
    };

    let Some(_listing_source_lock) =
        ListingSourceLock::try_acquire(&ctx.lock_manager, candidate.listing_source_id)
    else {
        debug!("Skipping URL because another worker is scraping this ListingSource");
        return Some(ScrapeCandidateOutcome {
            capture: None,
            errored: false,
            skipped: true,
            operational_failed: false,
        });
    };

    let scrape_result = tokio::select! {
        biased;
        _ = wait_for_scraper_stop(stop) => return None,
        _ = ctx.failure.notified() => return None,
        _ = ctx.command_tx.closed() => {
            ctx.failure.notify();
            return Some(ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                operational_failed: true,
            });
        }
        result = async {
            match candidate.fallback_currency {
                Some(fallback_currency) => {
                    ctx.scraper
                        .scrape_with_fallback_currency(
                            &candidate.listing_source_id,
                            &candidate.url,
                            candidate.url_pattern.as_deref(),
                            candidate.last_scraped_hash.as_deref(),
                            candidate.last_scraped_schema_fingerprint.as_deref(),
                            candidate.last_captured_raw_input_sha256.as_deref(),
                            Some(fallback_currency),
                        )
                        .await
                }
                None => {
                    ctx.scraper
                        .scrape(
                            &candidate.listing_source_id,
                            &candidate.url,
                            candidate.url_pattern.as_deref(),
                            candidate.last_scraped_hash.as_deref(),
                            candidate.last_scraped_schema_fingerprint.as_deref(),
                            candidate.last_captured_raw_input_sha256.as_deref(),
                        )
                        .await
                }
            }
        } => result,
    };

    // Keep both locks and await admitted metadata checkpoints through cancellation.
    // Only the owning runtime's hard deadline may abandon an unknown write result.
    Some(finish_scrape_candidate(&candidate, ctx, scrape_result).await)
}

async fn finish_scrape_candidate(
    candidate: &ScraperCandidate,
    ctx: &ScrapeDomainContext,
    scrape_result: Result<Option<ScrapedProduct>, ScraperError>,
) -> ScrapeCandidateOutcome {
    match scrape_result {
        Ok(Some(scraped)) => {
            let disposition = match &scraped.availability {
                product_listing_normalization::ListingAvailabilityQuickCheck::Resolved(
                    product_listing_core::listing_availability::ListingAvailability::SoldOut,
                ) => CrawlerDisposition::DormantSold,
                product_listing_normalization::ListingAvailabilityQuickCheck::Resolved(_)
                | product_listing_normalization::ListingAvailabilityQuickCheck::NoAssertion
                | product_listing_normalization::ListingAvailabilityQuickCheck::Unsupported => {
                    CrawlerDisposition::Active
                }
            };
            let provenance = match crawler_provenance(
                Some(scraped.hash.as_str()),
                Some(scraped.schema_fingerprint.as_str()),
            ) {
                Ok(provenance) => provenance,
                Err(error) => {
                    warn!(error = %error, listing_source_id = %candidate.listing_source_id, "Failed to build crawler raw-capture provenance");
                    return ScrapeCandidateOutcome {
                        capture: None,
                        errored: true,
                        skipped: false,
                        operational_failed: true,
                    };
                }
            };
            let meta = CandidateMeta {
                listing_source_id: candidate.listing_source_id,
                url: candidate.url.clone(),
                hash: scraped.hash,
                schema_fingerprint: scraped.schema_fingerprint,
                raw_input_sha256: scraped.raw_input_sha256,
                disposition,
                expected_last_captured_raw_input_sha256: candidate
                    .last_captured_raw_input_sha256
                    .clone(),
            };

            // Even equal local input can be stale after business commit/local mark failure.
            // Let the authoritative capture transaction establish Changed or Unchanged.
            ScrapeCandidateOutcome {
                capture: Some(RawCaptureRequest {
                    item: ProductListingRawCaptureItem::crawler(
                        candidate.listing_source_id,
                        &candidate.url,
                        scraped.raw_input,
                        provenance,
                    ),
                    on_success: RawCaptureSuccessAction::MarkScraped(meta),
                    on_failure: RawCaptureFailureAction::None,
                }),
                errored: false,
                skipped: false,
                operational_failed: false,
            }
        }
        Ok(None) => ScrapeCandidateOutcome {
            capture: None,
            errored: false,
            skipped: true,
            operational_failed: false,
        },
        Err(ScraperError::ProductListingRemoved { .. }) => handle_verified_removal(candidate),
        Err(e) => {
            let mut operational_failed = scraper_error_is_operational(&e);
            if operational_failed {
                ctx.failure.notify();
            }
            let error_message = e.to_string();
            let is_llm_budget_exceeded = matches!(&e, ScraperError::LlmBudgetExceeded { .. });
            let is_pending_schema_review = matches!(&e, ScraperError::PendingSchemaReview { .. });

            if let ScraperError::HttpError { kind, .. } = &e {
                let cooldown = durable_retry_cooldown_for(*kind);
                let next_retry_at = time::OffsetDateTime::now_utc()
                    + time::Duration::seconds(cooldown.as_secs() as i64);
                let status_code = match kind {
                    NetworkErrorKind::HttpStatus(code) => Some(*code as i32),
                    _ => None,
                };
                match ctx
                    .scraper_candidates
                    .mark_fetch_failure(
                        &candidate.listing_source_id,
                        &candidate.url,
                        &format!("{kind:?}"),
                        &error_message,
                        status_code,
                        next_retry_at,
                        candidate.last_captured_raw_input_sha256.as_deref(),
                    )
                    .await
                {
                    Ok(CrawlerUrlWriteOutcome::Applied) => {}
                    Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                        debug!(
                            listing_source_id = %candidate.listing_source_id,
                            url = %candidate.url,
                            crawler_url_write_outcome = "stale_noop",
                            "Skipped stale crawler fetch failure metadata"
                        );
                    }
                    Err(_) => {
                        operational_failed = true;
                        ctx.failure.notify();
                        warn!(
                            error_kind = "failure_metadata_write_failed",
                            "Failed to persist scraper fetch failure metadata"
                        );
                    }
                }
            } else {
                // Non-HTTP errors: schema failures, normalization errors, etc.
                // These do not affect retry scheduling but are persisted for observability.
                let error_kind = scraper_error_kind(&e);
                match &e {
                    ScraperError::SchemaRegenerationExhausted { .. }
                    | ScraperError::FreshSchemaNormalizationFailed { .. }
                    | ScraperError::SchemaClassificationRejected { .. }
                    | ScraperError::LlmBudgetExceeded { .. }
                    | ScraperError::PendingSchemaReview { .. } => {
                        let cooldown = std::time::Duration::from_secs(30 * 60);
                        let next_retry_at = time::OffsetDateTime::now_utc()
                            + time::Duration::seconds(cooldown.as_secs() as i64);
                        match ctx
                            .scraper_candidates
                            .mark_fetch_failure(
                                &candidate.listing_source_id,
                                &candidate.url,
                                error_kind,
                                &error_message,
                                None,
                                next_retry_at,
                                candidate.last_captured_raw_input_sha256.as_deref(),
                            )
                            .await
                        {
                            Ok(CrawlerUrlWriteOutcome::Applied) => {}
                            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                                debug!(
                                    listing_source_id = %candidate.listing_source_id,
                                    url = %candidate.url,
                                    crawler_url_write_outcome = "stale_noop",
                                    "Skipped stale crawler schema/classification cooldown metadata"
                                );
                            }
                            Err(_) => {
                                operational_failed = true;
                                ctx.failure.notify();
                                warn!(
                                    error_kind = "failure_metadata_write_failed",
                                    "Failed to persist schema/classification cooldown metadata"
                                );
                            }
                        }
                    }
                    _ => {
                        match ctx
                            .scraper_candidates
                            .mark_scraper_failure(
                                &candidate.listing_source_id,
                                &candidate.url,
                                error_kind,
                                &error_message,
                                candidate.last_captured_raw_input_sha256.as_deref(),
                            )
                            .await
                        {
                            Ok(CrawlerUrlWriteOutcome::Applied) => {}
                            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                                debug!(
                                    listing_source_id = %candidate.listing_source_id,
                                    url = %candidate.url,
                                    crawler_url_write_outcome = "stale_noop",
                                    "Skipped stale crawler failure metadata"
                                );
                            }
                            Err(_) => {
                                operational_failed = true;
                                ctx.failure.notify();
                                warn!(
                                    error_kind = "failure_metadata_write_failed",
                                    "Failed to persist scraper failure metadata"
                                );
                            }
                        }
                    }
                }
            }

            if operational_failed {
                return ScrapeCandidateOutcome {
                    capture: None,
                    errored: true,
                    skipped: false,
                    operational_failed,
                };
            }

            // Log LLM budget exhaustion at INFO level only once per ListingSource per batch
            if is_llm_budget_exceeded {
                if let ScraperError::LlmBudgetExceeded {
                    listing_source_id,
                    max_calls,
                    ..
                } = &e
                {
                    let mut exhausted = ctx.budget_exhausted_listing_sources.lock().await;
                    if exhausted.insert(*listing_source_id) {
                        info!(
                            listing_source_id = %listing_source_id,
                            max_calls,
                            "LLM call budget exhausted for ListingSource; skipping remaining URLs in batch"
                        );
                    }
                }
            } else if is_pending_schema_review {
                let mut pending = ctx.schema_pending_listing_sources.lock().await;
                if pending.insert(candidate.listing_source_id) {
                    info!(
                        listing_source_id = %candidate.listing_source_id,
                        "ProductListing schema review pending for ListingSource; skipping remaining URLs in batch"
                    );
                }
                warn!(error_kind = scraper_error_kind(&e), "Scraper run failed");
            } else if matches!(
                &e,
                ScraperError::ProductListingRemoved { .. } | ScraperError::NotProductPage { .. }
            ) {
                debug!(error_kind = scraper_error_kind(&e), "Scraper run failed");
            } else {
                warn!(error_kind = scraper_error_kind(&e), "Scraper run failed");
            }

            ScrapeCandidateOutcome {
                capture: None,
                errored: true,
                skipped: false,
                operational_failed,
            }
        }
    }
}

fn scraper_error_is_operational(error: &ScraperError) -> bool {
    use crate::scraper::css_selector::product_schema_service::ProductListingSchemaServiceError;
    use crate::scraper::normalization::error::NormalizationFailureScope;

    match error {
        ScraperError::RemovedPageSchemaDatabaseError(_)
        | ScraperError::SchemaServiceError(ProductListingSchemaServiceError::DatabaseError(_))
        | ScraperError::RawNormalizationInput(_)
        | ScraperError::SchemaFingerprint(_)
        | ScraperError::NoHost { .. } => true,
        ScraperError::NormalizationError(error) => {
            error.failure_scope() == NormalizationFailureScope::System
        }
        ScraperError::FreshSchemaNormalizationFailed {
            last_norm_error: error,
            ..
        } => error.failure_scope() == NormalizationFailureScope::System,
        ScraperError::HttpError { .. }
        | ScraperError::ProductListingRemoved { .. }
        | ScraperError::NotProductPage { .. }
        | ScraperError::SchemaClassificationRejected { .. }
        | ScraperError::SchemaRegenerationExhausted { .. }
        | ScraperError::LlmBudgetExceeded { .. }
        | ScraperError::PendingSchemaReview { .. }
        | ScraperError::SchemaServiceError(
            ProductListingSchemaServiceError::LargeLanguageModelError(_)
            | ProductListingSchemaServiceError::NoTextResponse(_)
            | ProductListingSchemaServiceError::StructuredResponseValidation { .. }
            | ProductListingSchemaServiceError::JsonParsingTargetSchemaError(_),
        ) => false,
    }
}

/// Returns a short, stable, machine-readable kind label for a [`ScraperError`].
///
/// These labels are persisted in `listing_source_urls.last_error_kind` so that
/// operators can filter / aggregate by error category without having to parse
/// the free-text message.  The `HttpError` variant is included for completeness
/// even though the caller currently only invokes this helper for non-HTTP errors.
fn scraper_error_kind(e: &ScraperError) -> &'static str {
    match e {
        ScraperError::HttpError { .. } => "HttpError",
        ScraperError::ProductListingRemoved { .. } => "ProductListingRemoved",
        ScraperError::NotProductPage { .. } => "NotProductPage",
        ScraperError::SchemaClassificationRejected { .. } => "SchemaClassificationRejected",
        ScraperError::NoHost { .. } => "NoHost",
        ScraperError::SchemaServiceError(_) => "SchemaServiceError",
        ScraperError::RemovedPageSchemaDatabaseError(_) => "RemovedPageSchemaDatabaseError",
        ScraperError::SchemaRegenerationExhausted { .. } => "SchemaRegenerationExhausted",
        ScraperError::FreshSchemaNormalizationFailed { .. } => "FreshSchemaNormalizationFailed",
        ScraperError::LlmBudgetExceeded { .. } => "LlmBudgetExceeded",
        ScraperError::NormalizationError(_) => "NormalizationError",
        ScraperError::RawNormalizationInput(_) => "RawNormalizationInput",
        ScraperError::SchemaFingerprint(_) => "SchemaFingerprint",
        ScraperError::PendingSchemaReview { .. } => "PendingSchemaReview",
    }
}

#[tracing::instrument(
    name = "crawler_scrape_domain_candidates",
    skip(candidates, ctx, stop),
    fields(candidate_count = candidates.len())
)]
async fn scrape_domain_candidates(
    candidates: Vec<ScraperCandidate>,
    ctx: ScrapeDomainContext,
    mut stop: watch::Receiver<bool>,
) -> ScrapeDomainOutcome {
    let mut outcome = ScrapeDomainOutcome::default();

    for candidate in candidates {
        let Some(candidate_outcome) = scrape_candidate(candidate, &ctx, &mut stop).await else {
            outcome.admission_stopped = true;
            break;
        };

        if candidate_outcome.operational_failed {
            outcome.failed += 1;
            outcome.operational_failed += 1;
            ctx.failure.notify();
            break;
        } else if candidate_outcome.errored {
            outcome.failed += 1;
        } else if let Some(pair) = candidate_outcome.capture {
            let enqueued = tokio::select! {
                biased;
                _ = ctx.failure.notified() => {
                    outcome.admission_stopped = true;
                    break;
                }
                enqueued = enqueue_raw_capture(&ctx.command_tx, pair, &mut stop) => enqueued,
            };
            match enqueued {
                Ok(Some(queue_wait)) => {
                    outcome.accepted += 1;

                    if queue_wait >= Duration::from_millis(10) {
                        warn!(
                            event = "crawler.raw_capture.enqueue_wait",
                            queue_wait_ms = queue_wait.as_millis(),
                            "Raw ProductListing capture queue applied backpressure"
                        );
                    }
                }
                Ok(None) => {
                    outcome.admission_stopped = true;
                    break;
                }
                Err(_) => {
                    error!("Command channel closed while scraper worker is running");
                    outcome.failed += 1;
                    outcome.operational_failed += 1;
                    ctx.failure.notify();
                    break;
                }
            }
        } else if candidate_outcome.skipped {
            outcome.skipped += 1;
        } else {
            // No capture and no deliberate skip is an unexplained producer output.
            outcome.failed += 1;
            outcome.operational_failed += 1;
            ctx.failure.notify();
            break;
        }
    }

    outcome
}

/// Poll concurrently with producers, never on their stop path. Every sender must be dropped
/// before drain can finish; cancelling this future cannot establish in-memory capture custody.
async fn run_raw_capture_collector_with_failure(
    mut command_rx: mpsc::Receiver<QueuedRawCapture>,
    raw_capture: Arc<dyn ProductListingRawCaptureService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    capture_batch_size: usize,
    capture_max_batch_age: Duration,
    failure: &OperationalFailureSignal,
) -> Result<RawCaptureDrainSummary, RawCaptureDrainError> {
    let capture_batch_size = capture_batch_size.max(1);
    let capture_max_batch_age = capture_max_batch_age.max(Duration::from_millis(1));
    let mut pending = Vec::<QueuedRawCapture>::with_capacity(capture_batch_size);
    let mut summary = RawCaptureDrainSummary::default();

    loop {
        let received =
            if let Some(oldest_enqueued_at) = pending.iter().map(|item| item.enqueued_at).min() {
                tokio::select! {
                    received = command_rx.recv() => received,
                    _ = tokio::time::sleep_until(oldest_enqueued_at + capture_max_batch_age) => {
                        summary.merge(flush_batch_with_failure(
                            &raw_capture,
                            &scraper_candidates,
                            std::mem::take(&mut pending),
                            command_rx.len(),
                            failure,
                        ).await);
                        continue;
                    }
                }
            } else {
                command_rx.recv().await
            };

        let Some(item) = received else { break };
        pending.push(item);
        if pending.len() >= capture_batch_size {
            summary.merge(
                flush_batch_with_failure(
                    &raw_capture,
                    &scraper_candidates,
                    std::mem::take(&mut pending),
                    command_rx.len(),
                    failure,
                )
                .await,
            );
        }
    }

    if !pending.is_empty() {
        summary.merge(
            flush_batch_with_failure(&raw_capture, &scraper_candidates, pending, 0, failure).await,
        );
    }

    if summary.is_complete() {
        info!(
            event = "crawler.raw_capture.drained",
            ?summary,
            "Raw capture channel drained"
        );
        Ok(summary)
    } else {
        warn!(
            event = "crawler.raw_capture.drain_incomplete",
            ?summary,
            "Raw capture drain left retryable or unconfirmed work"
        );
        Err(RawCaptureDrainError { summary })
    }
}

#[cfg(test)]
async fn run_raw_capture_collector(
    command_rx: mpsc::Receiver<QueuedRawCapture>,
    raw_capture: Arc<dyn ProductListingRawCaptureService>,
    scraper_candidates: Arc<dyn ScraperCandidateService>,
    capture_batch_size: usize,
    capture_max_batch_age: Duration,
) -> Result<RawCaptureDrainSummary, RawCaptureDrainError> {
    run_raw_capture_collector_with_failure(
        command_rx,
        raw_capture,
        scraper_candidates,
        capture_batch_size,
        capture_max_batch_age,
        &OperationalFailureSignal::default(),
    )
    .await
}

impl CrawlerCronJob {
    #[cfg(test)]
    #[tracing::instrument(name = "crawler_run_scraper_once", skip(self))]
    pub(super) async fn run_scraper_once(&self) {
        let outcome = self.run_scraper_pass().await;
        if outcome.is_complete() {
            info!(?outcome, "Scraper scheduler pass finished");
        } else {
            warn!(?outcome, "Scraper scheduler pass incomplete");
        }
    }

    /// Compatibility boundary for callers without a cooperative stop signal.
    #[cfg(test)]
    pub(super) async fn run_scraper_pass(&self) -> ScraperPassOutcome {
        let (keep_running, stop) = watch::channel(false);
        let outcome = self.run_scraper_pass_until(stop).await;
        drop(keep_running);
        outcome
    }

    /// True or sender loss stops admission and producer waits, then drains accepted captures.
    /// Runtime must retain/join this whole future and inspect the outcome before closing pools.
    /// An outer deadline or abort means unknown custody, never a successful drain.
    #[cfg(test)]
    pub(super) async fn run_scraper_pass_until(
        &self,
        stop: watch::Receiver<bool>,
    ) -> ScraperPassOutcome {
        self.run_scraper_pass_with_signal(stop, OperationalFailureSignal::default())
            .await
    }

    /// Operational failure latches `failure_stop` immediately for the job to relay.
    /// A private latch stops this pass independently. Return still joins every producer
    /// and all accepted collector work. The supplied sender must be monotonic.
    pub(super) async fn run_scraper_pass_until_with_failure(
        &self,
        stop: watch::Receiver<bool>,
        failure_stop: watch::Sender<bool>,
    ) -> ScraperPassOutcome {
        self.run_scraper_pass_with_signal(stop, OperationalFailureSignal::new(Some(failure_stop)))
            .await
    }

    async fn run_scraper_pass_with_signal(
        &self,
        mut stop: watch::Receiver<bool>,
        failure: OperationalFailureSignal,
    ) -> ScraperPassOutcome {
        let _unwind = NotifyFailureOnUnwind(failure.clone());
        if scraper_stop_requested(&stop) {
            return ScraperPassOutcome {
                admission_stopped: true,
                ..Default::default()
            };
        }
        let scraper_concurrency = self.config.scraper_concurrency;
        if scraper_concurrency == 0 {
            warn!(
                scraper_concurrency,
                "scraper_concurrency is 0, skipping scraper scheduler pass"
            );
            return ScraperPassOutcome::default();
        }
        let admitted = tokio::select! {
            biased;
            _ = wait_for_scraper_stop(&mut stop) => {
                return ScraperPassOutcome {
                    admission_stopped: true,
                    ..Default::default()
                };
            }
            admitted = self.admit_authoritative_scope_for_work("scraper") => admitted,
        };
        if !admitted {
            failure.notify();
            return ScraperPassOutcome {
                scope_refresh_failed: true,
                ..Default::default()
            };
        }

        let pass_start = tokio::time::Instant::now();
        info!(
            concurrency = scraper_concurrency,
            push_batch_size = self.config.effective_push_batch_size(),
            push_queue_capacity = self.config.effective_push_queue_capacity(),
            push_max_batch_age_ms = self.config.effective_push_max_batch_age().as_millis(),
            push_max_concurrency = self.config.effective_push_max_concurrency(),
            "Scraper scheduler pass starting"
        );
        let (command_tx, command_rx) =
            mpsc::channel::<QueuedRawCapture>(self.config.effective_push_queue_capacity());

        let mut unique_listing_source_ids = HashSet::new();
        // Unwind isolation replaces the old collector task boundary without detaching work.
        // The producer side observes receiver closure if the collector panics.
        let (mut outcome, captures) = tokio::join!(
            self.run_scraper_producers(
                command_tx,
                stop.clone(),
                &mut unique_listing_source_ids,
                &failure
            ),
            async {
                let _unwind = NotifyFailureOnUnwind(failure.clone());
                let captured = AssertUnwindSafe(run_raw_capture_collector_with_failure(
                    command_rx,
                    Arc::clone(&self.raw_capture),
                    Arc::clone(&self.scraper_candidates),
                    self.config.effective_push_batch_size(),
                    self.config.effective_push_max_batch_age(),
                    &failure,
                ))
                .catch_unwind()
                .await;
                if !matches!(captured, Ok(Ok(_))) {
                    failure.notify();
                }
                captured
            },
        );
        match captures {
            Ok(Ok(summary)) => outcome.captures = summary,
            Ok(Err(error)) => outcome.captures = error.summary,
            Err(_) => {
                error!(
                    error_kind = "collector_failed",
                    "Scraper raw capture collector failed; durable and unfinished counts are unknown"
                );
                outcome.collector_failed = true;
            }
        }

        outcome.operational_failure_notified = failure.is_notified();
        if outcome.has_operational_failure() {
            failure.notify();
        }
        outcome.admission_stopped |= scraper_stop_requested(&stop);
        let duration_ms = pass_start.elapsed().as_millis() as u64;
        #[cfg(not(test))]
        if !outcome.admission_stopped() {
            tokio::select! {
                biased;
                _ = wait_for_scraper_stop(&mut stop) => outcome.admission_stopped = true,
                usages = async {
                    self.scraper_candidates
                        .get_listing_source_llm_usage(unique_listing_source_ids.into_iter().collect())
                        .await
                } => match usages {
                    Ok(usages) => {
                        for usage in usages {
                            debug!(
                                listing_source_name = %usage.listing_source_name,
                                llm_calls_count = usage.llm_calls_count,
                                llm_calls_cap = self.config.scraper_max_llm_calls_per_listing_source,
                                llm_budget_exhausted = usage.llm_calls_count >= self.config.scraper_max_llm_calls_per_listing_source,
                                "ListingSource LLM usage summary"
                            );
                        }
                    }
                    Err(_) => {
                        warn!(error_kind = "llm_usage_lookup_failed", "Failed to load per-ListingSource LLM usage summary");
                    }
                },
            }
        }
        self.scraper_perf.record(outcome.total as u64, duration_ms);
        outcome
    }

    async fn run_scraper_producers(
        &self,
        command_tx: mpsc::Sender<QueuedRawCapture>,
        mut stop: watch::Receiver<bool>,
        unique_listing_source_ids: &mut HashSet<ListingSourceId>,
        failure: &OperationalFailureSignal,
    ) -> ScraperPassOutcome {
        let _unwind = NotifyFailureOnUnwind(failure.clone());
        let scraper_concurrency = self.config.scraper_concurrency;
        let mut seen_domains: HashSet<String> = HashSet::new();
        let mut active_domains: HashSet<String> = HashSet::new();
        let mut pending_domains: VecDeque<(String, Vec<ScraperCandidate>)> = VecDeque::new();
        let mut join_set: JoinSet<ScheduledScrapeDomainOutcome> = JoinSet::new();

        let budget_exhausted_listing_sources = Arc::new(Mutex::new(HashSet::new()));
        let schema_pending_listing_sources = Arc::new(Mutex::new(HashSet::new()));

        let mut outcome = ScraperPassOutcome::default();
        let mut no_more_candidates = false;

        // Keep the JoinSet outside the unwind boundary so even scheduler failure is joined.
        let admission = AssertUnwindSafe(async {
            'admission: loop {
                while join_set.len() < scraper_concurrency {
                    if scraper_stop_requested(&stop) {
                        outcome.admission_stopped = true;
                        break 'admission;
                    }
                    if failure.is_notified() {
                        break 'admission;
                    }
                    if command_tx.is_closed() {
                        failure.notify();
                        outcome.collector_failed = true;
                        break 'admission;
                    }
                    if let Some((domain, candidates)) = pending_domains
                        .iter()
                        .position(|(domain, _)| !active_domains.contains(domain))
                        .and_then(|idx| pending_domains.remove(idx))
                    {
                        let scraper = Arc::clone(&self.scraper_service);
                        let scraper_candidates = Arc::clone(&self.scraper_candidates);
                        let lock_manager = Arc::clone(&self.lock_manager);
                        let domain_tx = command_tx.clone();
                        let domain_stop = stop.clone();
                        let failure = failure.clone();
                        let budget_exhausted_listing_sources =
                            Arc::clone(&budget_exhausted_listing_sources);
                        let schema_pending_listing_sources =
                            Arc::clone(&schema_pending_listing_sources);
                        let span = tracing::info_span!("scrape_domain", domain = %domain);
                        active_domains.insert(domain.clone());
                        outcome.total += candidates.len();

                        join_set.spawn(
                            async move {
                                let _unwind = NotifyFailureOnUnwind(failure.clone());
                                let ctx = ScrapeDomainContext {
                                    scraper,
                                    scraper_candidates,
                                    lock_manager,
                                    command_tx: domain_tx,
                                    budget_exhausted_listing_sources,
                                    schema_pending_listing_sources,
                                    failure,
                                };

                                ScheduledScrapeDomainOutcome {
                                    domain,
                                    outcome: scrape_domain_candidates(candidates, ctx, domain_stop)
                                        .await,
                                }
                            }
                            .instrument(span),
                        );
                        continue;
                    }

                    if no_more_candidates {
                        break;
                    }

                    let mut excluded_domains: HashSet<String> = seen_domains.clone();
                    excluded_domains.extend(active_domains.iter().cloned());
                    excluded_domains.extend(
                        pending_domains
                            .iter()
                            .map(|(domain, _)| domain.to_ascii_lowercase()),
                    );
                    let excluded_domains: Vec<String> = excluded_domains.into_iter().collect();
                    let candidates = {
                        let lookup = async {
                            self.scraper_candidates
                                .get_candidates(
                                    self.config.effective_scraper_domain_batch_size() as i64,
                                    self.config.scraper_urls_per_domain.max(1),
                                    &excluded_domains,
                                )
                                .await
                        };
                        tokio::pin!(lookup);
                        loop {
                            tokio::select! {
                                biased;
                                _ = wait_for_scraper_stop(&mut stop) => {
                                    outcome.admission_stopped = true;
                                    break 'admission;
                                }
                                _ = failure.notified() => break 'admission,
                                _ = command_tx.closed() => {
                                    failure.notify();
                                    outcome.collector_failed = true;
                                    break 'admission;
                                }
                                joined = join_set.join_next(), if !join_set.is_empty() => {
                                    if let Some(joined) = joined {
                                        if let Ok(scheduled) = &joined {
                                            active_domains.remove(&scheduled.domain);
                                        }
                                        record_scraper_worker_outcome(&mut outcome, joined, failure);
                                        if outcome.worker_failed || outcome.admission_stopped {
                                            break 'admission;
                                        }
                                    }
                                }
                                result = &mut lookup => break result,
                            }
                        }
                    };
                    let candidates = match candidates {
                        Ok(candidates) => candidates,
                        Err(_) => {
                            warn!(
                                error_kind = "candidate_lookup_failed",
                                "Failed to retrieve scraper candidates"
                            );
                            outcome.candidate_lookup_failed = true;
                            failure.notify();
                            break 'admission;
                        }
                    };

                    if candidates.is_empty() {
                        no_more_candidates = true;
                        break;
                    }

                    let mut by_domain: HashMap<String, Vec<ScraperCandidate>> = HashMap::new();
                    for candidate in candidates {
                        unique_listing_source_ids.insert(candidate.listing_source_id);
                        let domain = candidate.url.host_str().unwrap_or("").to_ascii_lowercase();
                        seen_domains.insert(domain.clone());
                        by_domain.entry(domain).or_default().push(candidate);
                    }

                    if by_domain.is_empty() {
                        no_more_candidates = true;
                        break;
                    }

                    debug!(domains = by_domain.len(), "Candidates grouped by domain");
                    pending_domains.extend(by_domain);
                }

                if join_set.is_empty() {
                    break;
                }

                let joined = tokio::select! {
                    biased;
                    _ = wait_for_scraper_stop(&mut stop) => {
                        outcome.admission_stopped = true;
                        break;
                    }
                    _ = failure.notified() => break,
                    _ = command_tx.closed() => {
                        failure.notify();
                        outcome.collector_failed = true;
                        break;
                    }
                    joined = join_set.join_next() => joined,
                };
                if let Some(joined) = joined {
                    if let Ok(scheduled) = &joined {
                        active_domains.remove(&scheduled.domain);
                    }
                    record_scraper_worker_outcome(&mut outcome, joined, failure);
                    if outcome.worker_failed || outcome.admission_stopped {
                        break;
                    }
                }
            }
        })
        .catch_unwind()
        .await;
        if admission.is_err() {
            error!(error_kind = "admission_failed", "Scraper admission failed");
            outcome.worker_failed = true;
            failure.notify();
        }

        // Failure already latches cooperative stop. Never abort siblings: an admitted
        // metadata checkpoint still owes its actual result. Inspect every join, including
        // panics whose partial work remains unknown; retain the independent collector.
        while let Some(joined) = join_set.join_next().await {
            record_scraper_worker_outcome(&mut outcome, joined, failure);
        }
        drop(command_tx);
        outcome.admission_stopped |= scraper_stop_requested(&stop);

        outcome
    }
}

fn record_scraper_worker_outcome(
    outcome: &mut ScraperPassOutcome,
    joined: Result<ScheduledScrapeDomainOutcome, tokio::task::JoinError>,
    failure: &OperationalFailureSignal,
) {
    match joined {
        Ok(scheduled) => {
            outcome.accepted += scheduled.outcome.accepted;
            outcome.failed += scheduled.outcome.failed;
            outcome.operational_failed += scheduled.outcome.operational_failed;
            if scheduled.outcome.operational_failed != 0 {
                failure.notify();
            }
            outcome.skipped += scheduled.outcome.skipped;
            outcome.admission_stopped |= scheduled.outcome.admission_stopped;
        }
        Err(_) => {
            failure.notify();
            error!(
                error_kind = "worker_join_failed",
                "Scraper domain worker failed; unfinished URL count is unknown"
            );
            outcome.worker_failed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    mod cancellation {
        include!("scraper_cancellation_tests.rs");
    }

    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::scraper_service::{MockScraperService, ScrapedProduct};
    use crate::service::cron::config::CrawlerCronConfig;
    use crate::service::cron::test_support::{noop_listing_source_registration, scraper_candidate};
    use crate::service::listing_source_registration::{
        ListingSourceRegistrationService, ListingSourceSyncError,
        MockListingSourceRegistrationRepository, MockListingSourceRegistrationSource,
    };
    use crate::service::raw_capture::MockProductListingRawCaptureService;
    use crate::spider::advisory_lock::LocalLockManager;
    use crate::spider::candidate_service::MockSpiderCandidateService;
    use crate::spider::service::MockSpiderService;
    use listing_source_core::ListingSourceId;

    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    type ScraperCandidateResultFuture =
        Pin<Box<dyn Future<Output = Result<Vec<ScraperCandidate>, sqlx::Error>> + Send>>;

    fn empty_spider_dependencies() -> (MockSpiderCandidateService, MockSpiderService) {
        let mut spider_candidates = MockSpiderCandidateService::new();
        spider_candidates
            .expect_get_candidates()
            .returning(|_, _| Box::pin(async { Ok(vec![]) }));

        (spider_candidates, MockSpiderService::new())
    }

    fn no_raw_capture_service() -> Box<MockProductListingRawCaptureService> {
        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture.expect_capture().times(0);
        Box::new(raw_capture)
    }

    fn item(listing_source_id: ListingSourceId, product_id: &str) -> ProductListingRawCaptureItem {
        let url = url::Url::parse(&format!("https://example.test/products/{product_id}"))
            .unwrap_or_else(|error| panic!("test URL: {error}"));
        let input = crawler_verified_removal_input(&url)
            .unwrap_or_else(|error| panic!("test removal input: {error}"));
        let provenance = crawler_provenance(None, None)
            .unwrap_or_else(|error| panic!("test provenance: {error}"));
        ProductListingRawCaptureItem::crawler(listing_source_id, &url, input, provenance)
    }

    fn meta(listing_source_id: ListingSourceId, url: &str, hash: &str) -> CandidateMeta {
        CandidateMeta {
            listing_source_id,
            url: url::Url::parse(url).unwrap(),
            hash: hash.to_owned(),
            schema_fingerprint: "schema-fingerprint".to_owned(),
            raw_input_sha256: vec![3; 32],
            disposition: CrawlerDisposition::Active,
            expected_last_captured_raw_input_sha256: None,
        }
    }

    fn queued(item: ProductListingRawCaptureItem, meta: CandidateMeta) -> QueuedRawCapture {
        QueuedRawCapture {
            request: (item, meta).into(),
            enqueued_at: tokio::time::Instant::now(),
        }
    }

    #[tokio::test]
    async fn should_mark_only_the_matching_successful_push_input_as_scraped() {
        let first_listing_source_id = ListingSourceId::new();
        let second_listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://first.example/product").unwrap();
        let observed_raw_input_sha256 = vec![9; 32];
        let observed_raw_input_sha256_for_mark = observed_raw_input_sha256.clone();
        let mut push_service = MockProductListingRawCaptureService::new();
        push_service.expect_capture().once().returning(|products| {
            assert_eq!(products.len(), 2);
            Box::pin(async {
                vec![
                    ProductListingRawCaptureOutcome::Persisted,
                    ProductListingRawCaptureOutcome::RetryableFailure,
                ]
            })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .withf(
                move |listing_source_id,
                      url,
                      hash,
                      _,
                      _,
                      _,
                      expected_last_captured_raw_input_sha256| {
                    *listing_source_id == first_listing_source_id
                        && url == &first_url
                        && hash == "first"
                        && expected_last_captured_raw_input_sha256.as_deref()
                            == Some(observed_raw_input_sha256_for_mark.as_slice())
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let push_service: Arc<dyn ProductListingRawCaptureService> = Arc::new(push_service);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        let mut first_meta = meta(
            first_listing_source_id,
            "https://first.example/product",
            "first",
        );
        first_meta.expected_last_captured_raw_input_sha256 = Some(observed_raw_input_sha256);

        flush_batch(
            &push_service,
            &scraper_candidates,
            vec![
                queued(item(first_listing_source_id, "same-product-id"), first_meta),
                queued(
                    item(second_listing_source_id, "same-product-id"),
                    meta(
                        second_listing_source_id,
                        "https://second.example/product",
                        "second",
                    ),
                ),
            ],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_not_mark_missing_source_capture_as_persisted_or_failed() {
        let listing_source_id = ListingSourceId::new();
        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture.expect_capture().once().returning(|products| {
            assert_eq!(products.len(), 1);
            Box::pin(async { vec![ProductListingRawCaptureOutcome::DiscardedMissingSource] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_mark_as_scraped().never();
        scraper_candidates.expect_mark_scraper_failure().never();

        let raw_capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(raw_capture);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        flush_batch(
            &raw_capture,
            &scraper_candidates,
            vec![queued(
                item(listing_source_id, "deleted-source"),
                meta(
                    listing_source_id,
                    "https://deleted.example/product",
                    "deleted-source",
                ),
            )],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_default_missing_raw_capture_results_to_failure() {
        let first_listing_source_id = ListingSourceId::new();
        let second_listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://first.example/product").unwrap();
        let mut push_service = MockProductListingRawCaptureService::new();
        push_service.expect_capture().once().returning(|products| {
            assert_eq!(products.len(), 2);
            Box::pin(async { vec![ProductListingRawCaptureOutcome::Persisted] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .withf(
                move |listing_source_id,
                      url,
                      hash,
                      _,
                      _,
                      _,
                      expected_last_captured_raw_input_sha256| {
                    *listing_source_id == first_listing_source_id
                        && url == &first_url
                        && hash == "first"
                        && expected_last_captured_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let push_service: Arc<dyn ProductListingRawCaptureService> = Arc::new(push_service);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);

        flush_batch(
            &push_service,
            &scraper_candidates,
            vec![
                queued(
                    item(first_listing_source_id, "first"),
                    meta(
                        first_listing_source_id,
                        "https://first.example/product",
                        "first",
                    ),
                ),
                queued(
                    item(second_listing_source_id, "second"),
                    meta(
                        second_listing_source_id,
                        "https://second.example/product",
                        "second",
                    ),
                ),
            ],
            0,
        )
        .await;
    }

    struct CaptureUseCase {
        calls: Arc<AtomicUsize>,
        commit_failed: bool,
    }

    #[async_trait::async_trait]
    impl product_listing_service::use_cases::CaptureProductListingRawObservationUseCase
        for CaptureUseCase
    {
        async fn execute(
            &self,
            _: &application::operation_context::OperationContext,
            _: product_listing_service::use_cases::CaptureProductListingRawObservationCommand,
        ) -> Result<
            product_listing_service::use_cases::CaptureProductListingRawObservationResult,
            product_listing_service::use_cases::CaptureProductListingRawObservationError,
        > {
            use product_listing_service::use_cases::{
                CaptureProductListingRawObservationError as Error,
                CaptureProductListingRawObservationResult as Outcome,
            };
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if self.commit_failed {
                return Err(Error::CommitTransactionFailed);
            }
            let product_listing_raw_stream_id =
                product_listing_service::ports::ProductListingRawStreamId::new();
            if call == 0 {
                Ok(Outcome::Changed {
                    product_listing_raw_stream_id,
                    product_listing_raw_revision_id:
                        product_listing_service::ports::ProductListingRawRevisionId::new(),
                    revision: 1,
                })
            } else {
                Ok(Outcome::Unchanged {
                    product_listing_raw_stream_id,
                    latest_revision: 1,
                })
            }
        }
    }

    #[tokio::test]
    async fn should_not_mark_done_when_capture_transaction_commit_fails() {
        let calls = Arc::new(AtomicUsize::new(0));
        let capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(
            crate::service::raw_capture::ProductListingRawCaptureServiceImpl::new(
                Arc::new(CaptureUseCase {
                    calls: calls.clone(),
                    commit_failed: true,
                }),
                1,
            ),
        );
        let mut candidates = MockScraperCandidateService::new();
        candidates.expect_mark_as_scraped().never();
        candidates.expect_touch_scraped().never();
        candidates.expect_mark_removed().never();
        let candidates: Arc<dyn ScraperCandidateService> = Arc::new(candidates);
        let id = ListingSourceId::new();
        let summary = flush_batch(
            &capture,
            &candidates,
            vec![queued(
                item(id, "commit-failed"),
                meta(id, "https://example.test/products/commit-failed", "page"),
            )],
            0,
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(summary.accepted, 1);
        assert_eq!(summary.durable, 0);
        assert_eq!(summary.completed, 0);
        assert_eq!(summary.capture_failed, 1);
        assert!(!summary.is_complete());
    }

    #[tokio::test]
    async fn should_reconfirm_equal_input_when_business_commit_succeeds_but_local_mark_fails() {
        let calls = Arc::new(AtomicUsize::new(0));
        let capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(
            crate::service::raw_capture::ProductListingRawCaptureServiceImpl::new(
                Arc::new(CaptureUseCase {
                    calls: calls.clone(),
                    commit_failed: false,
                }),
                1,
            ),
        );
        let marks = Arc::new(AtomicUsize::new(0));
        let mut candidates = MockScraperCandidateService::new();
        let calls_for_mark = calls.clone();
        let marks_for_mock = marks.clone();
        candidates.expect_mark_as_scraped().times(2).returning(
            move |_, _, _, _, _, _, expected| {
                assert_eq!(expected, Some(&[3; 32][..]));
                let mark = marks_for_mock.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    calls_for_mark.load(Ordering::SeqCst),
                    mark + 1,
                    "each mark follows authoritative capture, including retry"
                );
                Box::pin(async move {
                    if mark == 0 {
                        Err(sqlx::Error::PoolClosed)
                    } else {
                        Ok(CrawlerUrlWriteOutcome::Applied)
                    }
                })
            },
        );
        candidates.expect_touch_scraped().never();
        let mut scraper = MockScraperService::new();
        scraper.expect_scrape().times(2).returning(|_, url, _, _, _, _| {
            let raw_input = crawler_verified_removal_input(url).unwrap();
            Box::pin(async move { Ok(Some(ScrapedProduct {
                raw_input,
                availability: product_listing_normalization::ListingAvailabilityQuickCheck::NoAssertion,
                hash: "same-page".to_owned(),
                schema_fingerprint: "same-schema".to_owned(),
                raw_input_sha256: vec![3; 32],
            })) })
        });
        let (ctx, _rx) = scrape_candidate_context(candidates, scraper);
        let (_stop_tx, mut stop) = watch::channel(false);
        let id = ListingSourceId::new();
        for attempt in 0..2 {
            let mut candidate = scraper_candidate(
                "Source",
                url::Url::parse("https://example.test/products/retry").unwrap(),
            );
            candidate.listing_source_id = id;
            candidate.last_scraped_hash = Some("same-page".to_owned());
            candidate.last_scraped_schema_fingerprint = Some("same-schema".to_owned());
            candidate.last_captured_raw_input_sha256 = Some(vec![3; 32]);
            let request = scrape_candidate(candidate, &ctx, &mut stop)
                .await
                .expect("not stopped")
                .capture
                .expect("equal input must reach authoritative capture");
            let summary = flush_batch(
                &capture,
                &ctx.scraper_candidates,
                vec![QueuedRawCapture {
                    request,
                    enqueued_at: tokio::time::Instant::now(),
                }],
                0,
            )
            .await;
            assert_eq!(summary.durable, 1);
            assert_eq!(summary.completed, attempt);
            assert_eq!(summary.local_mark_failed, 1 - attempt);
            assert_eq!(summary.is_complete(), attempt == 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn should_report_acceptance_without_claiming_durable_completion() {
        let mut scraper = MockScraperService::new();
        scraper
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::ProductListingRemoved {
                        url,
                        details: "test removal".into(),
                    })
                })
            });
        let mut candidates = MockScraperCandidateService::new();
        candidates.expect_mark_removed().never();
        let (ctx, mut rx) = scrape_candidate_context(candidates, scraper);
        let (_stop_tx, stop) = watch::channel(false);
        let summary = scrape_domain_candidates(
            vec![scraper_candidate(
                "Source",
                url::Url::parse("https://example.test/products/accepted").unwrap(),
            )],
            ctx,
            stop,
        )
        .await;
        assert_eq!(summary.accepted, 1);
        assert_eq!(summary.failed, 0);
        assert!(rx.recv().await.is_some());
        assert!(rx.recv().await.is_none());
    }

    #[rstest::rstest]
    #[case::zero_limit(0, vec![1, 1, 1, 1, 1])]
    #[case::capped(2, vec![2, 2, 1])]
    #[tokio::test]
    async fn should_drain_all_accepted_batches_after_failure_and_producer_stop(
        #[case] batch_size: usize,
        #[case] expected_batch_sizes: Vec<usize>,
    ) {
        let batch_sizes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let batch_sizes_for_mock = batch_sizes.clone();
        let mut capture = MockProductListingRawCaptureService::new();
        capture.expect_capture().returning(move |items| {
            let mut sizes = batch_sizes_for_mock.lock().unwrap();
            let first = sizes.is_empty();
            sizes.push(items.len());
            let mut outcomes = vec![ProductListingRawCaptureOutcome::Persisted; items.len()];
            if first {
                outcomes[0] = ProductListingRawCaptureOutcome::RetryableFailure;
            }
            Box::pin(async move { outcomes })
        });
        let mut candidates = MockScraperCandidateService::new();
        candidates
            .expect_mark_as_scraped()
            .times(4)
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        let (tx, rx) = mpsc::channel(5);
        let collector = tokio::spawn(run_raw_capture_collector(
            rx,
            Arc::new(capture),
            Arc::new(candidates),
            batch_size,
            Duration::from_secs(60),
        ));
        let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
        let producer = tokio::spawn(async move {
            let (_stop_tx, mut stop) = watch::channel(false);
            let id = ListingSourceId::new();
            for index in 0..5 {
                enqueue_raw_capture(
                    &tx,
                    (
                        item(id, &index.to_string()),
                        meta(id, "https://example.test/products/drain", "page"),
                    ),
                    &mut stop,
                )
                .await
                .unwrap();
            }
            accepted_tx.send(()).unwrap();
            std::future::pending::<()>().await;
            drop(tx);
        });
        accepted_rx.await.unwrap();
        producer.abort();
        assert!(producer.await.unwrap_err().is_cancelled());
        let error = tokio::time::timeout(Duration::from_secs(2), collector)
            .await
            .expect("collector must survive producer stop")
            .expect("collector must join")
            .expect_err("first capture failed");
        assert_eq!(*batch_sizes.lock().unwrap(), expected_batch_sizes);
        assert_eq!(error.summary.accepted, 5);
        assert_eq!(error.summary.durable, 4);
        assert_eq!(error.summary.completed, 4);
        assert_eq!(error.summary.capture_failed, 1);
    }

    #[tokio::test]
    async fn should_surface_collector_panic_in_pass_outcome() {
        let mut candidates = MockScraperCandidateService::new();
        candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Source",
                    url::Url::parse("https://example.test/products/panic").unwrap(),
                )]
            }));
        let mut scraper = MockScraperService::new();
        scraper
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::ProductListingRemoved {
                        url,
                        details: "test removal".into(),
                    })
                })
            });
        let mut capture = MockProductListingRawCaptureService::new();
        capture
            .expect_capture()
            .once()
            .returning(|_| Box::pin(async { panic!("injected collector failure") }));
        let mut job = scraper_job(CrawlerCronConfig::default(), candidates, scraper);
        job.raw_capture = Arc::new(capture);
        let outcome = job.run_scraper_pass().await;
        assert_eq!(outcome.accepted, 1);
        assert_eq!(outcome.captures.completed, 0);
        assert!(outcome.collector_failed);
        assert!(!outcome.is_complete());
    }

    #[tokio::test]
    async fn should_surface_failed_capture_in_pass_outcome() {
        let mut candidates = MockScraperCandidateService::new();
        candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Source",
                    url::Url::parse("https://example.test/products/failure").unwrap(),
                )]
            }));
        candidates
            .expect_mark_scraper_failure()
            .once()
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        candidates.expect_mark_removed().never();
        let mut scraper = MockScraperService::new();
        scraper
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::ProductListingRemoved {
                        url,
                        details: "test removal".into(),
                    })
                })
            });
        let mut capture = MockProductListingRawCaptureService::new();
        capture.expect_capture().once().returning(|_| {
            Box::pin(async { vec![ProductListingRawCaptureOutcome::RetryableFailure] })
        });
        let mut job = scraper_job(CrawlerCronConfig::default(), candidates, scraper);
        job.raw_capture = Arc::new(capture);
        let outcome = job.run_scraper_pass().await;
        assert_eq!(outcome.accepted, 1);
        assert_eq!(outcome.captures.capture_failed, 1);
        assert_eq!(outcome.captures.completed, 0);
        assert!(!outcome.is_complete());
    }

    #[tokio::test]
    async fn should_apply_backpressure_when_raw_capture_queue_is_full() {
        let (_stop_tx, mut stop) = watch::channel(false);
        let (command_tx, mut command_rx) = mpsc::channel::<QueuedRawCapture>(1);
        let listing_source_id = ListingSourceId::new();

        enqueue_raw_capture(
            &command_tx,
            (
                item(listing_source_id, "first"),
                meta(
                    listing_source_id,
                    "https://example.com/product/first",
                    "first",
                ),
            ),
            &mut stop,
        )
        .await
        .expect("first enqueue must fit");

        let second_tx = command_tx.clone();
        let second = tokio::spawn(async move {
            enqueue_raw_capture(
                &second_tx,
                (
                    item(listing_source_id, "second"),
                    meta(
                        listing_source_id,
                        "https://example.com/product/second",
                        "second",
                    ),
                ),
                &mut stop,
            )
            .await
        });

        tokio::task::yield_now().await;
        assert!(
            !second.is_finished(),
            "second enqueue must wait while the bounded queue is full"
        );

        assert!(command_rx.recv().await.is_some());

        let result = tokio::time::timeout(Duration::from_secs(1), second)
            .await
            .expect("second enqueue must unblock")
            .expect("second enqueue task must join");

        assert!(result.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn should_flush_partial_batch_at_maximum_age() {
        let (_stop_tx, mut stop) = watch::channel(false);
        let push_calls = Arc::new(AtomicUsize::new(0));
        let push_calls_for_mock = Arc::clone(&push_calls);

        let mut push_service = MockProductListingRawCaptureService::new();
        push_service
            .expect_capture()
            .once()
            .returning(move |products| {
                push_calls_for_mock.fetch_add(1, Ordering::SeqCst);
                let len = products.len();
                Box::pin(async move { vec![ProductListingRawCaptureOutcome::Persisted; len] })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .once()
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let (tx, rx) = mpsc::channel(2);
        let collector = tokio::spawn(run_raw_capture_collector(
            rx,
            Arc::new(push_service),
            Arc::new(scraper_candidates),
            10,
            Duration::from_secs(5),
        ));

        let listing_source_id = ListingSourceId::new();
        enqueue_raw_capture(
            &tx,
            (
                item(listing_source_id, "one"),
                meta(listing_source_id, "https://example.com/product/one", "one"),
            ),
            &mut stop,
        )
        .await
        .expect("enqueue must succeed");

        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;

        assert_eq!(push_calls.load(Ordering::SeqCst), 1);

        drop(tx);
        let summary = collector
            .await
            .expect("collector task must join")
            .expect("drain must complete");
        assert_eq!(summary.accepted, 1);
        assert_eq!(summary.durable, 1);
        assert_eq!(summary.completed, 1);
    }

    #[tokio::test]
    async fn should_flush_final_partial_batch_when_raw_capture_channel_closes() {
        let (_stop_tx, mut stop) = watch::channel(false);
        let push_calls = Arc::new(AtomicUsize::new(0));
        let push_calls_for_mock = Arc::clone(&push_calls);

        let mut push_service = MockProductListingRawCaptureService::new();
        push_service
            .expect_capture()
            .once()
            .returning(move |products| {
                assert_eq!(products.len(), 2);
                push_calls_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async {
                    vec![
                        ProductListingRawCaptureOutcome::Persisted,
                        ProductListingRawCaptureOutcome::Persisted,
                    ]
                })
            });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_as_scraped()
            .times(2)
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let (tx, rx) = mpsc::channel(2);
        let collector = tokio::spawn(run_raw_capture_collector(
            rx,
            Arc::new(push_service),
            Arc::new(scraper_candidates),
            10,
            Duration::from_secs(5),
        ));

        let listing_source_id = ListingSourceId::new();
        enqueue_raw_capture(
            &tx,
            (
                item(listing_source_id, "one"),
                meta(listing_source_id, "https://example.com/product/one", "one"),
            ),
            &mut stop,
        )
        .await
        .expect("first enqueue must succeed");
        enqueue_raw_capture(
            &tx,
            (
                item(listing_source_id, "two"),
                meta(listing_source_id, "https://example.com/product/two", "two"),
            ),
            &mut stop,
        )
        .await
        .expect("second enqueue must succeed");

        drop(tx);
        let summary = collector
            .await
            .expect("collector task must join")
            .expect("drain must complete");
        assert_eq!(summary.accepted, 2);
        assert_eq!(summary.durable, 2);
        assert_eq!(summary.completed, 2);

        assert_eq!(push_calls.load(Ordering::SeqCst), 1);
    }

    fn get_candidates_once_by_domain<F>(
        build_candidates: F,
    ) -> impl Fn(i64, i64, &[String]) -> ScraperCandidateResultFuture + Send + Sync + 'static
    where
        F: Fn() -> Vec<ScraperCandidate> + Send + Sync + 'static,
    {
        move |_, _, excluded_domains| {
            let excluded_domains: HashSet<String> = excluded_domains.iter().cloned().collect();
            let candidates = build_candidates();
            Box::pin(async move {
                Ok(candidates
                    .into_iter()
                    .filter(|candidate| {
                        candidate
                            .url
                            .host_str()
                            .map(|domain| !excluded_domains.contains(&domain.to_ascii_lowercase()))
                            .unwrap_or(false)
                    })
                    .collect())
            })
        }
    }

    fn scraper_job(
        config: CrawlerCronConfig,
        scraper_candidates: MockScraperCandidateService,
        scraper_service: MockScraperService,
    ) -> CrawlerCronJob {
        let (spider_candidates, spider_service) = empty_spider_dependencies();

        CrawlerCronJob::new(
            config,
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            no_raw_capture_service(),
        )
    }

    fn scrape_candidate_context(
        scraper_candidates: MockScraperCandidateService,
        scraper_service: MockScraperService,
    ) -> (ScrapeDomainContext, mpsc::Receiver<QueuedRawCapture>) {
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(1);

        (
            ScrapeDomainContext {
                scraper: Arc::new(scraper_service),
                scraper_candidates: Arc::new(scraper_candidates),
                lock_manager: Arc::new(LocalLockManager::new()),
                command_tx,
                budget_exhausted_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                schema_pending_listing_sources: Arc::new(Mutex::new(HashSet::new())),
                failure: OperationalFailureSignal::default(),
            },
            command_rx,
        )
    }

    #[tokio::test]
    async fn should_skip_scraper_candidates_when_authoritative_scope_refresh_fails() {
        let mut source = MockListingSourceRegistrationSource::new();
        source
            .expect_fetch_registered_listing_sources()
            .once()
            .returning(|| {
                Box::pin(async {
                    Err(ListingSourceSyncError::FetchError(
                        "business unavailable".to_owned(),
                    ))
                })
            });
        let registration = ListingSourceRegistrationService::new(
            Box::new(source),
            Box::new(MockListingSourceRegistrationRepository::new()),
        );
        let (spider_candidates, spider_service) = empty_spider_dependencies();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::new(LocalLockManager::new()),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(MockScraperCandidateService::new()),
            Box::new(MockScraperService::new()),
            registration,
            no_raw_capture_service(),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_run_scraper_candidates_and_push_products() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test ListingSource",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(None) }));

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_apply_durable_retry_cooldown_after_final_fetch_failure() {
        let before = time::OffsetDateTime::now_utc();
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test ListingSource",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |_, _, _, _, status_code, next_retry_at, expected_raw_input_sha256| {
                    let expected_cooldown = durable_retry_cooldown_for(NetworkErrorKind::Timeout);
                    let expected_from =
                        before + time::Duration::seconds(expected_cooldown.as_secs() as i64);
                    let expected_until = time::OffsetDateTime::now_utc()
                        + time::Duration::seconds(expected_cooldown.as_secs() as i64)
                        + time::Duration::seconds(1);
                    status_code.is_none()
                        && *next_retry_at >= expected_from
                        && *next_retry_at <= expected_until
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: crate::network::policy::NetworkErrorKind::Timeout,
                        details: "timeout".to_string(),
                    })
                })
            });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_mark_removed_url_active_only_after_raw_capture() {
        let url = url::Url::parse("https://example.com/product/removed").unwrap();
        let candidate = scraper_candidate("ListingSource", url.clone());
        let listing_source_id = candidate.listing_source_id;
        let outcome = handle_verified_removal(&candidate);
        let request = outcome.capture.expect("removal must be queued for capture");

        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture
            .expect_capture()
            .once()
            .withf(move |observations| {
                observations.len() == 1
                    && observations[0].command.listing_source_id == listing_source_id
                    && observations[0].command.source_record_key
                        == "https://example.com/product/removed"
                    && observations[0].command.input.operation()
                        == product_listing_normalization::RawProductListingOperation::Delete
            })
            .returning(|_| Box::pin(async { vec![ProductListingRawCaptureOutcome::Persisted] }));

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_removed()
            .once()
            .withf(
                move |source_id, candidate_url, raw_input_sha256, expected_raw_input_sha256| {
                    *source_id == listing_source_id
                        && candidate_url.as_str() == "https://example.com/product/removed"
                        && raw_input_sha256.len() == 32
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));

        let raw_capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(raw_capture);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        flush_batch(
            &raw_capture,
            &scraper_candidates,
            vec![QueuedRawCapture {
                request,
                enqueued_at: tokio::time::Instant::now(),
            }],
            0,
        )
        .await;
    }

    #[rstest::rstest]
    #[case::sold(product_listing_core::listing_availability::ListingAvailability::SoldOut)]
    #[case::active(product_listing_core::listing_availability::ListingAvailability::Available)]
    #[tokio::test]
    async fn should_queue_observation_when_previous_raw_input_hash_matches(
        #[case] availability: product_listing_core::listing_availability::ListingAvailability,
    ) {
        let url = url::Url::parse("https://example.com/product/sold").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url.clone());
        candidate.last_captured_raw_input_sha256 = Some(vec![3; 32]);

        let raw_input = crawler_verified_removal_input(&url)
            .unwrap_or_else(|error| panic!("test raw input: {error}"));
        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().once().return_once(
            move |_, _, _, _, _, expected_raw_input_sha256| {
                assert_eq!(expected_raw_input_sha256, Some(&[3; 32][..]));
                Box::pin(async move {
                    Ok(Some(ScrapedProduct {
                        raw_input,
                        availability:
                            product_listing_normalization::ListingAvailabilityQuickCheck::Resolved(
                                availability,
                            ),
                        hash: "sold-hash".to_owned(),
                        schema_fingerprint: "sold-schema".to_owned(),
                        raw_input_sha256: vec![3; 32],
                    }))
                })
            },
        );

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates.expect_mark_as_scraped().never();
        let (ctx, _rx) = scrape_candidate_context(scraper_candidates, scraper_service);
        let (_stop_tx, mut stop) = watch::channel(false);
        let outcome = scrape_candidate(candidate, &ctx, &mut stop)
            .await
            .expect("not stopped");

        let request = outcome
            .capture
            .expect("equal local input must queue raw capture");
        let RawCaptureSuccessAction::MarkScraped(meta) = request.on_success else {
            panic!("scrape must queue a completion");
        };
        assert_eq!(
            meta.expected_last_captured_raw_input_sha256.as_deref(),
            Some(&[3_u8; 32][..])
        );
        assert!(!outcome.errored);
        assert!(!outcome.skipped);
    }

    #[tokio::test]
    async fn should_keep_removed_url_active_when_raw_capture_fails() {
        let url = url::Url::parse("https://example.com/product/removed").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url);
        let expected_last_captured_raw_input_sha256 = vec![5; 32];
        candidate.last_captured_raw_input_sha256 =
            Some(expected_last_captured_raw_input_sha256.clone());
        let outcome = handle_verified_removal(&candidate);
        let request = outcome.capture.expect("removal must be queued for capture");

        let mut raw_capture = MockProductListingRawCaptureService::new();
        raw_capture.expect_capture().once().returning(|_| {
            Box::pin(async { vec![ProductListingRawCaptureOutcome::RetryableFailure] })
        });

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_scraper_failure()
            .once()
            .withf(move |_, _, kind, message, received_raw_input_sha256| {
                kind == "RawCaptureFailed"
                    && message == "verified removal raw capture did not commit"
                    && *received_raw_input_sha256
                        == Some(expected_last_captured_raw_input_sha256.as_slice())
            })
            .returning(|_, _, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
        scraper_candidates.expect_mark_removed().never();

        let raw_capture: Arc<dyn ProductListingRawCaptureService> = Arc::new(raw_capture);
        let scraper_candidates: Arc<dyn ScraperCandidateService> = Arc::new(scraper_candidates);
        flush_batch(
            &raw_capture,
            &scraper_candidates,
            vec![QueuedRawCapture {
                request,
                enqueued_at: tokio::time::Instant::now(),
            }],
            0,
        )
        .await;
    }

    #[tokio::test]
    async fn should_mark_same_domain_500_fetch_failure() {
        let url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let mut candidate = scraper_candidate("ListingSource", url.clone());
        let expected_last_captured_raw_input_sha256 = vec![6; 32];
        candidate.last_captured_raw_input_sha256 =
            Some(expected_last_captured_raw_input_sha256.clone());
        let listing_source_id = candidate.listing_source_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |received_listing_source_id,
                      received_url,
                      _,
                      _,
                      status_code,
                      _,
                      received_raw_input_sha256| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &url
                        && *status_code == Some(500)
                        && *received_raw_input_sha256
                            == Some(expected_last_captured_raw_input_sha256.as_slice())
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(500),
                        details: "internal server error".to_string(),
                    })
                })
            });

        let (ctx, _rx) = scrape_candidate_context(scraper_candidates, scraper_service);
        let (_stop_tx, mut stop) = watch::channel(false);
        let outcome = scrape_candidate(candidate, &ctx, &mut stop)
            .await
            .expect("not stopped");

        assert!(outcome.errored);
    }

    #[tokio::test]
    async fn should_mark_same_domain_429_fetch_failure() {
        let url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let candidate = scraper_candidate("ListingSource", url.clone());
        let listing_source_id = candidate.listing_source_id;

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |received_listing_source_id,
                      received_url,
                      _,
                      _,
                      status_code,
                      _,
                      expected_raw_input_sha256| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &url
                        && *status_code == Some(429)
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::HttpError {
                        url,
                        kind: NetworkErrorKind::HttpStatus(429),
                        details: "too many requests".to_string(),
                    })
                })
            });

        let (ctx, _rx) = scrape_candidate_context(scraper_candidates, scraper_service);
        let (_stop_tx, mut stop) = watch::channel(false);
        let outcome = scrape_candidate(candidate, &ctx, &mut stop)
            .await
            .expect("not stopped");

        assert!(outcome.errored);
    }

    #[tokio::test]
    async fn should_continue_same_domain_after_500_failure() {
        let first_url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let second_url = url::Url::parse("https://same-domain.com/product/2").unwrap();
        let first_candidate_url = first_url.clone();
        let second_candidate_url = second_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let first_candidate_url = first_candidate_url.clone();
                let second_candidate_url = second_candidate_url.clone();
                vec![
                    scraper_candidate("ListingSource", first_candidate_url),
                    scraper_candidate("ListingSource", second_candidate_url),
                ]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |_, received_url, _, _, status_code, _, expected_raw_input_sha256| {
                    received_url == &first_url
                        && *status_code == Some(500)
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let scrape_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(move |_, url, _, _, _, _| {
                let url = url.clone();
                let attempt = scrape_count_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt == 0 {
                        Err(ScraperError::HttpError {
                            url,
                            kind: crate::network::policy::NetworkErrorKind::HttpStatus(500),
                            details: "internal server error".to_string(),
                        })
                    } else {
                        Ok(None)
                    }
                })
            });

        let job = scraper_job(
            CrawlerCronConfig {
                scraper_domain_delay: Duration::ZERO,
                ..CrawlerCronConfig::default()
            },
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;

        assert_eq!(scrape_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn should_continue_same_domain_after_retryable_network_failure() {
        let first_url = url::Url::parse("https://same-domain.com/product/1").unwrap();
        let second_url = url::Url::parse("https://same-domain.com/product/2").unwrap();
        let first_candidate_url = first_url.clone();
        let second_candidate_url = second_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let first_candidate_url = first_candidate_url.clone();
                let second_candidate_url = second_candidate_url.clone();
                vec![
                    scraper_candidate("ListingSource", first_candidate_url),
                    scraper_candidate("ListingSource", second_candidate_url),
                ]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |_, received_url, _, _, status_code, _, expected_raw_input_sha256| {
                    received_url == &first_url
                        && *status_code == Some(429)
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let scrape_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scrape_count_for_mock = Arc::clone(&scrape_count);
        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(move |_, url, _, _, _, _| {
                let url = url.clone();
                let attempt = scrape_count_for_mock.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    if attempt == 0 {
                        Err(ScraperError::HttpError {
                            url,
                            kind: crate::network::policy::NetworkErrorKind::HttpStatus(429),
                            details: "too many requests".to_string(),
                        })
                    } else {
                        Ok(None)
                    }
                })
            });

        let job = scraper_job(
            CrawlerCronConfig {
                scraper_domain_delay: Duration::from_millis(1),
                ..CrawlerCronConfig::default()
            },
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
        assert_eq!(scrape_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn should_mark_fetch_failure_for_llm_budget_exceeded_error() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test ListingSource",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|listing_source_id, url, _, _, _, _| {
                let url = url.clone();
                let listing_source_id = *listing_source_id;
                Box::pin(async move {
                    Err(ScraperError::LlmBudgetExceeded {
                        listing_source_id,
                        url,
                        max_calls: 5,
                    })
                })
            });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_skip_remaining_listing_source_candidates_when_schema_review_is_pending() {
        let listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://example.com/product/1").unwrap();
        let second_url = url::Url::parse("https://example.com/product/2").unwrap();

        let first_url_for_candidates = first_url.clone();
        let second_url_for_candidates = second_url.clone();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let mut first =
                    scraper_candidate("Test ListingSource", first_url_for_candidates.clone());
                first.listing_source_id = listing_source_id;
                let mut second =
                    scraper_candidate("Test ListingSource", second_url_for_candidates.clone());
                second.listing_source_id = listing_source_id;
                vec![first, second]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |received_listing_source_id,
                      received_url,
                      kind,
                      _,
                      _,
                      _,
                      expected_raw_input_sha256| {
                    *received_listing_source_id == listing_source_id
                        && received_url == &first_url
                        && kind == "PendingSchemaReview"
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::PendingSchemaReview {
                        url,
                        review_id: crate::CrawlerReviewId::new(),
                    })
                })
            });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    /// `FreshSchemaNormalizationFailed` must be handled identically to
    /// `SchemaRegenerationExhausted`: write a cooldown via `mark_fetch_failure`
    /// so the URL is held back until the backoff window expires.
    #[tokio::test]
    async fn should_mark_fetch_failure_for_fresh_schema_normalization_failure() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![scraper_candidate(
                    "Test ListingSource",
                    url::Url::parse("https://example.com/product/1").unwrap(),
                )]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::FreshSchemaNormalizationFailed {
                        url,
                        attempts: 3,
                        last_norm_error: Box::new(
                            crate::scraper::normalization::product_normalization_service::NormalizationError::TitleEmpty,
                        ),
                    })
                })
            });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_mark_fetch_failure_for_schema_classification_rejection() {
        let before = time::OffsetDateTime::now_utc();
        let url = url::Url::parse("https://example.com/product/1").unwrap();
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain({
                let url = url.clone();
                move || vec![scraper_candidate("Test ListingSource", url.clone())]
            }));
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(
                move |_,
                      received_url,
                      kind,
                      _,
                      status_code,
                      next_retry_at,
                      expected_raw_input_sha256| {
                    *received_url == url
                        && kind == "SchemaClassificationRejected"
                        && status_code.is_none()
                        && *next_retry_at > before
                        && expected_raw_input_sha256.is_none()
                },
            )
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
        scraper_candidates.expect_mark_scraper_failure().never();

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, url, _, _, _, _| {
                let url = url.clone();
                Box::pin(async move {
                    Err(ScraperError::SchemaClassificationRejected {
                        url,
                        details: "removed classification requires HIGH confidence".to_string(),
                    })
                })
            });

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_scrape_candidates_from_multiple_domains() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![
                    scraper_candidate(
                        "ListingSource A",
                        url::Url::parse("https://domain-a.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "ListingSource B",
                        url::Url::parse("https://domain-b.com/product/2").unwrap(),
                    ),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(2)
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(None) }));

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_refill_scraper_domain_slot_while_slow_domain_is_running() {
        let slow_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let fast_url = url::Url::parse("https://domain-b.com/product/1").unwrap();
        let refill_url = url::Url::parse("https://domain-c.com/product/1").unwrap();

        let slow_url_for_candidates = slow_url.clone();
        let fast_url_for_candidates = fast_url.clone();
        let refill_url_for_candidates = refill_url.clone();
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(move |_, _, excluded_domains| {
                let excluded_domains = excluded_domains.to_vec();
                let slow_url = slow_url_for_candidates.clone();
                let fast_url = fast_url_for_candidates.clone();
                let refill_url = refill_url_for_candidates.clone();
                Box::pin(async move {
                    if excluded_domains.is_empty() {
                        Ok(vec![
                            scraper_candidate("Slow", slow_url),
                            scraper_candidate("Fast", fast_url),
                        ])
                    } else if excluded_domains.contains(&"domain-a.com".to_string())
                        && excluded_domains.contains(&"domain-b.com".to_string())
                        && !excluded_domains.contains(&"domain-c.com".to_string())
                    {
                        Ok(vec![scraper_candidate("Refill", refill_url)])
                    } else {
                        Ok(vec![])
                    }
                })
            });

        let slow_running = Arc::new(AtomicBool::new(false));
        let refill_started_while_slow_running = Arc::new(AtomicBool::new(false));
        let release_slow = Arc::new(tokio::sync::Notify::new());
        let release_slow_for_mock = Arc::clone(&release_slow);
        let slow_running_for_mock = Arc::clone(&slow_running);
        let refill_started_for_mock = Arc::clone(&refill_started_while_slow_running);

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(move |_, url, _, _, _, _| {
                let url = url.clone();
                let release_slow = Arc::clone(&release_slow_for_mock);
                let slow_running = Arc::clone(&slow_running_for_mock);
                let refill_started = Arc::clone(&refill_started_for_mock);
                let slow_url = slow_url.clone();
                let fast_url = fast_url.clone();
                let refill_url = refill_url.clone();
                Box::pin(async move {
                    if url == slow_url {
                        slow_running.store(true, Ordering::SeqCst);
                        release_slow.notified().await;
                        slow_running.store(false, Ordering::SeqCst);
                    } else if url == fast_url {
                        while !slow_running.load(Ordering::SeqCst) {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                        }
                    } else if url == refill_url {
                        if slow_running.load(Ordering::SeqCst) {
                            refill_started.store(true, Ordering::SeqCst);
                        }
                        release_slow.notify_one();
                    }
                    Ok(None)
                })
            });

        let job = scraper_job(
            CrawlerCronConfig {
                scraper_concurrency: 2,
                scraper_urls_per_domain: 100,
                scraper_domain_delay: Duration::ZERO,
                ..CrawlerCronConfig::default()
            },
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;

        assert!(refill_started_while_slow_running.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn should_skip_same_listing_source_candidate_already_scraping_on_another_domain() {
        let listing_source_id = ListingSourceId::new();
        let first_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let second_url = url::Url::parse("https://domain-b.com/product/2").unwrap();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let mut first = scraper_candidate("Same ListingSource", first_url.clone());
                first.listing_source_id = listing_source_id;
                let mut second = scraper_candidate("Same ListingSource", second_url.clone());
                second.listing_source_id = listing_source_id;
                vec![first, second]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .once()
            .returning(|_, _, _, _, _, _| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok(None)
                })
            });

        let job = scraper_job(
            CrawlerCronConfig {
                scraper_concurrency: 2,
                scraper_domain_delay: Duration::ZERO,
                ..CrawlerCronConfig::default()
            },
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_skip_scraper_candidate_when_url_lock_is_already_held() {
        let locked_url = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let open_url = url::Url::parse("https://domain-a.com/product/2").unwrap();

        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(move || {
                let locked_url = locked_url.clone();
                let open_url = open_url.clone();
                vec![
                    scraper_candidate("ListingSource A", locked_url),
                    scraper_candidate("ListingSource A", open_url),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(1)
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(None) }));

        let lock_manager = Arc::new(LocalLockManager::new());
        let prelocked = url::Url::parse("https://domain-a.com/product/1").unwrap();
        let _prelock = UrlLock::try_acquire(&lock_manager, &prelocked).unwrap();
        let (spider_candidates, spider_service) = empty_spider_dependencies();

        let job = CrawlerCronJob::new(
            CrawlerCronConfig::default(),
            Arc::clone(&lock_manager),
            Box::new(spider_candidates),
            Box::new(spider_service),
            Box::new(scraper_candidates),
            Box::new(scraper_service),
            noop_listing_source_registration(),
            no_raw_capture_service(),
        );

        job.run_scraper_once().await;
    }

    #[tokio::test]
    async fn should_scrape_all_urls_from_same_domain() {
        let mut scraper_candidates = MockScraperCandidateService::new();
        scraper_candidates
            .expect_get_candidates()
            .returning(get_candidates_once_by_domain(|| {
                vec![
                    scraper_candidate(
                        "ListingSource",
                        url::Url::parse("https://same-domain.com/product/1").unwrap(),
                    ),
                    scraper_candidate(
                        "ListingSource",
                        url::Url::parse("https://same-domain.com/product/2").unwrap(),
                    ),
                    scraper_candidate(
                        "ListingSource",
                        url::Url::parse("https://same-domain.com/product/3").unwrap(),
                    ),
                ]
            }));

        let mut scraper_service = MockScraperService::new();
        scraper_service
            .expect_scrape()
            .times(3)
            .returning(|_, _, _, _, _, _| Box::pin(async { Ok(None) }));

        let job = scraper_job(
            CrawlerCronConfig::default(),
            scraper_candidates,
            scraper_service,
        );

        job.run_scraper_once().await;
    }
}

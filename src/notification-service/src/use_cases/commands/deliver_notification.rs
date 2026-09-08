use crate::ports::{
    notification_channel_sender::{
        NotificationChannelSendError, NotificationDeliveryDispatchError,
        NotificationDeliveryDispatcher,
    },
    notification_delivery_repository::{
        ClaimNotificationDeliveryOutcome, ClaimedNotificationDelivery, NotificationDeliveryError,
        NotificationDeliveryRepository,
    },
};
use application::error::box_error;
use notification_core::{
    notification_delivery::NotificationDeliveryChannel,
    notification_delivery_id::NotificationDeliveryId,
};
use std::time::Duration as StdDuration;
use time::{Duration, OffsetDateTime};
use tokio::time::{Instant, sleep_until, timeout_at};
use uuid::Uuid;

const DELIVERY_LEASE_DURATION: Duration = Duration::minutes(5);
const UNREGISTERED_CHANNEL_ERROR_CODE: &str = "NOTIFICATION_CHANNEL_UNREGISTERED";
const MAX_ATTEMPT_TIMEOUT: StdDuration = StdDuration::from_secs(4 * 60);
const CLAIM_RACE_RETRY_DELAY: StdDuration = StdDuration::from_secs(1);

/// One monotonic budget covers claim, target/provider work, finalization, and backoff.
/// Keep at least one minute between this budget and the fixed five-minute claim lease.
#[derive(Debug, Clone, Copy)]
pub struct DeliverNotificationTiming {
    attempt_timeout: StdDuration,
    operation_timeout: StdDuration,
    initial_retry_delay: StdDuration,
    max_retry_delay: StdDuration,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "delivery timing requires nonzero durations, attempt <= four minutes, operation < attempt, and initial retry <= max retry < attempt"
)]
pub struct InvalidDeliverNotificationTiming;

impl DeliverNotificationTiming {
    pub fn new(
        attempt_timeout: StdDuration,
        operation_timeout: StdDuration,
        initial_retry_delay: StdDuration,
        max_retry_delay: StdDuration,
    ) -> Result<Self, InvalidDeliverNotificationTiming> {
        if [
            attempt_timeout,
            operation_timeout,
            initial_retry_delay,
            max_retry_delay,
        ]
        .iter()
        .any(StdDuration::is_zero)
            || attempt_timeout > MAX_ATTEMPT_TIMEOUT
            || operation_timeout >= attempt_timeout
            || initial_retry_delay > max_retry_delay
            || max_retry_delay >= attempt_timeout
        {
            return Err(InvalidDeliverNotificationTiming);
        }
        Ok(Self {
            attempt_timeout,
            operation_timeout,
            initial_retry_delay,
            max_retry_delay,
        })
    }
}

impl Default for DeliverNotificationTiming {
    fn default() -> Self {
        Self {
            attempt_timeout: MAX_ATTEMPT_TIMEOUT,
            operation_timeout: StdDuration::from_secs(30),
            initial_retry_delay: StdDuration::from_millis(100),
            max_retry_delay: StdDuration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAttemptPhase {
    Claim,
    Provider,
    Finalization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliverNotificationCommand {
    pub notification_delivery_id: NotificationDeliveryId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverNotificationResult {
    Delivered {
        attempt_count: u32,
    },
    DeliveryMissing,
    AlreadyDelivered,
    /// Nonterminal: defer until this persisted lease expires; never acknowledge.
    AlreadyClaimed {
        lease_expires_at: OffsetDateTime,
    },
    /// Nonterminal: a status/expiry race made the row reclaimable. Never acknowledge.
    ClaimDeferred {
        retry_after: StdDuration,
    },
    SourceMissing,
    PermanentlyFailed,
}

enum DeliveryCompletion {
    Delivered {
        provider_message_id: String,
        completed_at: OffsetDateTime,
    },
    RetryableFailure {
        error_code: &'static str,
        completed_at: OffsetDateTime,
    },
    PermanentFailure {
        error_code: &'static str,
        completed_at: OffsetDateTime,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum DeliverNotificationError {
    #[error("notification delivery repository operation failed")]
    Repository(#[from] NotificationDeliveryError),
    #[error("notification delivery send failed temporarily")]
    RetryableSend(#[source] NotificationChannelSendError),
    #[error("notification delivery provider acceptance is unknown")]
    AmbiguousSend(#[source] NotificationChannelSendError),
    #[error("notification delivery attempt timed out during {phase:?}")]
    AttemptTimedOut { phase: DeliveryAttemptPhase },
    #[error("notification delivery finalization budget exhausted; completion is unconfirmed")]
    FinalizationExhausted {
        #[source]
        source: NotificationDeliveryError,
    },
    #[error("notification channel sender is not registered for {channel:?}")]
    UnregisteredChannel {
        channel: NotificationDeliveryChannel,
    },

    #[error("notification delivery lease was lost before finalization")]
    LeaseLost,
}

#[async_trait::async_trait]
/// Only terminal results permit acknowledgment. Errors and both claim deferrals do not.
/// Dropping execute cancels this attempt; no detached send/finalization work is spawned.
pub trait DeliverNotificationUseCase: Send + Sync {
    async fn execute(
        &self,
        command: DeliverNotificationCommand,
    ) -> Result<DeliverNotificationResult, DeliverNotificationError>;
}

pub struct DeliverNotificationHandler<R> {
    deliveries: R,
    dispatcher: NotificationDeliveryDispatcher,
    timing: DeliverNotificationTiming,
}

impl<R> DeliverNotificationHandler<R> {
    pub fn new(deliveries: R, dispatcher: NotificationDeliveryDispatcher) -> Self {
        Self::with_timing(deliveries, dispatcher, DeliverNotificationTiming::default())
    }

    pub fn with_timing(
        deliveries: R,
        dispatcher: NotificationDeliveryDispatcher,
        timing: DeliverNotificationTiming,
    ) -> Self {
        Self {
            deliveries,
            dispatcher,
            timing,
        }
    }

    fn operation_deadline(&self, deadline: Instant) -> Instant {
        deadline.min(Instant::now() + self.timing.operation_timeout)
    }
}

impl<R> DeliverNotificationHandler<R>
where
    R: NotificationDeliveryRepository,
{
    async fn finalize(
        &self,
        claimed: &ClaimedNotificationDelivery,
        completion: &DeliveryCompletion,
        deadline: Instant,
    ) -> Result<(), DeliverNotificationError> {
        let mut retry_delay = self.timing.initial_retry_delay;
        let mut retry_attempt = 1_u32;

        loop {
            if Instant::now() >= deadline {
                return Err(DeliverNotificationError::AttemptTimedOut {
                    phase: DeliveryAttemptPhase::Finalization,
                });
            }
            let operation = async {
                match completion {
                    DeliveryCompletion::Delivered {
                        provider_message_id,
                        completed_at,
                    } => {
                        self.deliveries
                            .mark_delivered(
                                claimed.notification_delivery_id,
                                claimed.lease_token,
                                provider_message_id,
                                *completed_at,
                            )
                            .await
                    }
                    DeliveryCompletion::RetryableFailure {
                        error_code,
                        completed_at,
                    } => {
                        self.deliveries
                            .mark_retryable_failure(
                                claimed.notification_delivery_id,
                                claimed.lease_token,
                                error_code,
                                *completed_at,
                            )
                            .await
                    }
                    DeliveryCompletion::PermanentFailure {
                        error_code,
                        completed_at,
                    } => {
                        self.deliveries
                            .mark_permanent_failure(
                                claimed.notification_delivery_id,
                                claimed.lease_token,
                                error_code,
                                *completed_at,
                            )
                            .await
                    }
                }
            };
            let result = timeout_at(self.operation_deadline(deadline), operation)
                .await
                .unwrap_or_else(|source| {
                    Err(NotificationDeliveryError::OperationFailed {
                        source: box_error(source),
                    })
                });

            match result {
                Ok(true) => return Ok(()),
                Ok(false) => return Err(DeliverNotificationError::LeaseLost),
                Err(error @ NotificationDeliveryError::OperationFailed { .. }) => {
                    let retry_at = Instant::now() + retry_delay;
                    if retry_at >= deadline {
                        return Err(DeliverNotificationError::FinalizationExhausted {
                            source: error,
                        });
                    }
                    tracing::warn!(
                        notification_delivery_id = %claimed.notification_delivery_id,
                        retry_attempt,
                        retry_delay_ms = retry_delay.as_millis() as u64,
                        "notification delivery finalization failed; retrying"
                    );
                    sleep_until(retry_at).await;
                    if Instant::now() >= deadline {
                        return Err(DeliverNotificationError::FinalizationExhausted {
                            source: error,
                        });
                    }
                    retry_delay = retry_delay
                        .saturating_mul(2)
                        .min(self.timing.max_retry_delay);
                    retry_attempt = retry_attempt.saturating_add(1);
                }
                Err(error) => return Err(DeliverNotificationError::Repository(error)),
            }
        }
    }
}

#[async_trait::async_trait]
impl<R> DeliverNotificationUseCase for DeliverNotificationHandler<R>
where
    R: NotificationDeliveryRepository,
{
    #[tracing::instrument(
        name = "deliver_notification",
        skip_all,
        fields(notification_delivery_id = %command.notification_delivery_id)
    )]
    async fn execute(
        &self,
        command: DeliverNotificationCommand,
    ) -> Result<DeliverNotificationResult, DeliverNotificationError> {
        let deadline = Instant::now() + self.timing.attempt_timeout;
        let now = OffsetDateTime::now_utc();
        let lease_token = Uuid::now_v7();
        let (claimed, source) = match timeout_at(
            self.operation_deadline(deadline),
            self.deliveries.claim_and_load_source(
                command.notification_delivery_id,
                now,
                now + DELIVERY_LEASE_DURATION,
                lease_token,
            ),
        )
        .await
        .map_err(|_| DeliverNotificationError::AttemptTimedOut {
            phase: DeliveryAttemptPhase::Claim,
        })?? {
            ClaimNotificationDeliveryOutcome::Missing => {
                return Ok(DeliverNotificationResult::DeliveryMissing);
            }
            ClaimNotificationDeliveryOutcome::Delivered => {
                return Ok(DeliverNotificationResult::AlreadyDelivered);
            }
            ClaimNotificationDeliveryOutcome::PermanentlyFailed => {
                return Ok(DeliverNotificationResult::PermanentlyFailed);
            }
            ClaimNotificationDeliveryOutcome::AlreadyClaimed { lease_expires_at } => {
                return Ok(DeliverNotificationResult::AlreadyClaimed { lease_expires_at });
            }
            ClaimNotificationDeliveryOutcome::Reclaimable => {
                return Ok(DeliverNotificationResult::ClaimDeferred {
                    retry_after: CLAIM_RACE_RETRY_DELAY,
                });
            }
            ClaimNotificationDeliveryOutcome::Claimed { delivery, source } => (delivery, source),
        };

        let remaining_lease =
            StdDuration::try_from(claimed.lease_expires_at - OffsetDateTime::now_utc())
                .map_err(|_| DeliverNotificationError::LeaseLost)?;
        if remaining_lease.is_zero() {
            return Err(DeliverNotificationError::LeaseLost);
        }
        let deadline =
            deadline.min(Instant::now() + remaining_lease.min(self.timing.attempt_timeout));
        if Instant::now() >= deadline {
            return Err(DeliverNotificationError::AttemptTimedOut {
                phase: DeliveryAttemptPhase::Claim,
            });
        }

        let Some(source) = *source else {
            let completion = DeliveryCompletion::PermanentFailure {
                error_code: "NOTIFICATION_SOURCE_MISSING",
                completed_at: OffsetDateTime::now_utc(),
            };
            self.finalize(&claimed, &completion, deadline).await?;
            return Ok(DeliverNotificationResult::SourceMissing);
        };

        // Timeout is ambiguous: acceptance may precede the lost response. Keep PROCESSING
        // rather than releasing the lease for an immediate resend.
        match timeout_at(
            self.operation_deadline(deadline),
            self.dispatcher.dispatch(&source),
        )
        .await
        .map_err(|_| DeliverNotificationError::AttemptTimedOut {
            phase: DeliveryAttemptPhase::Provider,
        })? {
            Ok(sent) => {
                let completion = DeliveryCompletion::Delivered {
                    provider_message_id: sent.provider_message_id,
                    completed_at: OffsetDateTime::now_utc(),
                };
                self.finalize(&claimed, &completion, deadline).await?;
                Ok(DeliverNotificationResult::Delivered {
                    attempt_count: claimed.attempt_count,
                })
            }
            Err(NotificationDeliveryDispatchError::UnregisteredChannel { channel }) => {
                let completion = DeliveryCompletion::PermanentFailure {
                    error_code: UNREGISTERED_CHANNEL_ERROR_CODE,
                    completed_at: OffsetDateTime::now_utc(),
                };
                self.finalize(&claimed, &completion, deadline).await?;
                Err(DeliverNotificationError::UnregisteredChannel { channel })
            }
            Err(NotificationDeliveryDispatchError::Send(
                error @ NotificationChannelSendError::Ambiguous { .. },
            )) => Err(DeliverNotificationError::AmbiguousSend(error)),
            Err(NotificationDeliveryDispatchError::Send(
                error @ NotificationChannelSendError::Retryable { .. },
            )) => {
                let completion = DeliveryCompletion::RetryableFailure {
                    error_code: error.code(),
                    completed_at: OffsetDateTime::now_utc(),
                };
                self.finalize(&claimed, &completion, deadline).await?;
                Err(DeliverNotificationError::RetryableSend(error))
            }
            Err(NotificationDeliveryDispatchError::Send(
                error @ NotificationChannelSendError::Permanent { .. },
            )) => {
                let completion = DeliveryCompletion::PermanentFailure {
                    error_code: error.code(),
                    completed_at: OffsetDateTime::now_utc(),
                };
                self.finalize(&claimed, &completion, deadline).await?;
                Ok(DeliverNotificationResult::PermanentlyFailed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        notification_channel_sender::{
            NotificationChannelSender, NotificationDeliveryDispatcher,
            NotificationDeliveryDispatcherRegistrationError, SentNotificationDelivery,
        },
        notification_delivery_repository::{
            ClaimedNotificationDelivery, NotificationDeliverySource,
        },
    };
    use application::error::box_error;
    use listing_source_core::ListingSourceName;
    use localization::Language;
    use notification_core::notification_id::NotificationId;
    use notification_core::{
        notification::{
            NotificationContent, PartnershipApplicationDecision,
            PartnershipApplicationNotificationSnapshot,
        },
        notification_delivery::{NotificationDeliveryChannel, NotificationDeliveryTargetKey},
    };
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use party_core::party_name::PartyName;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use user_core::user_id::UserId;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum FinalizationCall {
        Delivered {
            lease_token: Uuid,
            provider_message_id: String,
            completed_at: OffsetDateTime,
        },
        RetryableFailure {
            lease_token: Uuid,
            error_code: String,
            completed_at: OffsetDateTime,
        },
        PermanentFailure {
            lease_token: Uuid,
            error_code: String,
            completed_at: OffsetDateTime,
        },
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    enum PersistedState {
        #[default]
        Processing,
        Delivered,
        Pending,
        Failed,
    }

    #[derive(Default)]
    struct DeliveryState {
        claimed_delivery_ids: Mutex<Vec<NotificationDeliveryId>>,
        claimed_lease_tokens: Mutex<Vec<Uuid>>,
        delivered_message_ids: Mutex<Vec<String>>,
        permanent_failure_codes: Mutex<Vec<String>>,
        finalization_calls: Mutex<Vec<FinalizationCall>>,
        finalization_failures_remaining: Mutex<usize>,
        finalization_response_losses_remaining: Mutex<usize>,
        persisted_completion: Mutex<Option<FinalizationCall>>,
        finalization_cancellations: AtomicUsize,
        persisted_state: Mutex<PersistedState>,
    }

    struct FakeDeliveryRepository {
        claimed: ClaimedNotificationDelivery,
        source: NotificationDeliverySource,
        state: Arc<DeliveryState>,
        claim_delay: StdDuration,
        claim_hangs: bool,
        source_missing: bool,
        finalization_behavior: FinalizationBehavior,
    }

    #[derive(Clone, Copy, Default)]
    enum FinalizationBehavior {
        #[default]
        Normal,
        Hang,
        CommitThenHang,
        LeaseLost,
        Invalid,
    }

    struct CancellationProbe<'a>(&'a AtomicUsize);

    impl Drop for CancellationProbe<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl FakeDeliveryRepository {
        fn new(
            notification_delivery_id: NotificationDeliveryId,
            source: NotificationDeliverySource,
            state: Arc<DeliveryState>,
        ) -> Self {
            Self {
                claimed: ClaimedNotificationDelivery {
                    notification_delivery_id,
                    notification_id: source.notification_id,
                    lease_token: Uuid::now_v7(),
                    lease_expires_at: OffsetDateTime::now_utc() + DELIVERY_LEASE_DURATION,
                    attempt_count: 2,
                },
                source,
                state,
                claim_delay: StdDuration::ZERO,
                claim_hangs: false,
                source_missing: false,
                finalization_behavior: FinalizationBehavior::Normal,
            }
        }

        async fn record_finalization(
            &self,
            call: FinalizationCall,
            persisted_state: PersistedState,
        ) -> Result<bool, NotificationDeliveryError> {
            self.state
                .finalization_calls
                .lock()
                .map_err(|_| repository_error())?
                .push(call.clone());
            match self.finalization_behavior {
                FinalizationBehavior::Hang => {
                    let _probe = CancellationProbe(&self.state.finalization_cancellations);
                    return std::future::pending().await;
                }
                FinalizationBehavior::LeaseLost => return Ok(false),
                FinalizationBehavior::Invalid => {
                    return Err(NotificationDeliveryError::InvalidPersistedState {
                        source: box_error(std::io::Error::other("test invalid state")),
                    });
                }
                FinalizationBehavior::Normal | FinalizationBehavior::CommitThenHang => {}
            }
            fail_next_finalization(&self.state)?;
            let newly_committed = {
                let mut persisted = self
                    .state
                    .persisted_completion
                    .lock()
                    .map_err(|_| repository_error())?;
                if let Some(original) = persisted.as_ref() {
                    if original != &call {
                        return Ok(false);
                    }
                    false
                } else {
                    match &call {
                        FinalizationCall::Delivered {
                            provider_message_id,
                            ..
                        } => self
                            .state
                            .delivered_message_ids
                            .lock()
                            .map_err(|_| repository_error())?
                            .push(provider_message_id.clone()),
                        FinalizationCall::PermanentFailure { error_code, .. } => self
                            .state
                            .permanent_failure_codes
                            .lock()
                            .map_err(|_| repository_error())?
                            .push(error_code.clone()),
                        FinalizationCall::RetryableFailure { .. } => {}
                    }
                    *persisted = Some(call);
                    *self
                        .state
                        .persisted_state
                        .lock()
                        .map_err(|_| repository_error())? = persisted_state;
                    true
                }
            };
            if newly_committed
                && matches!(
                    self.finalization_behavior,
                    FinalizationBehavior::CommitThenHang
                )
            {
                let _probe = CancellationProbe(&self.state.finalization_cancellations);
                return std::future::pending().await;
            }
            let mut losses = self
                .state
                .finalization_response_losses_remaining
                .lock()
                .map_err(|_| repository_error())?;
            if *losses > 0 {
                *losses -= 1;
                return Err(repository_error());
            }
            Ok(true)
        }
    }

    fn fail_next_finalization(state: &DeliveryState) -> Result<(), NotificationDeliveryError> {
        let mut remaining = state
            .finalization_failures_remaining
            .lock()
            .map_err(|_| repository_error())?;
        if *remaining == 0 {
            return Ok(());
        }
        *remaining -= 1;
        Err(repository_error())
    }

    #[async_trait::async_trait]
    impl NotificationDeliveryRepository for FakeDeliveryRepository {
        async fn claim_and_load_source(
            &self,
            notification_delivery_id: NotificationDeliveryId,
            _: OffsetDateTime,
            _: OffsetDateTime,
            lease_token: Uuid,
        ) -> Result<ClaimNotificationDeliveryOutcome, NotificationDeliveryError> {
            if self.claim_hangs {
                return std::future::pending().await;
            }
            if !self.claim_delay.is_zero() {
                tokio::time::sleep(self.claim_delay).await;
            }
            self.state
                .claimed_delivery_ids
                .lock()
                .map_err(|_| repository_error())?
                .push(notification_delivery_id);
            self.state
                .claimed_lease_tokens
                .lock()
                .map_err(|_| repository_error())?
                .push(lease_token);
            *self
                .state
                .persisted_state
                .lock()
                .map_err(|_| repository_error())? = PersistedState::Processing;
            let mut claimed = self.claimed.clone();
            claimed.lease_token = lease_token;
            Ok(ClaimNotificationDeliveryOutcome::Claimed {
                delivery: claimed,
                source: Box::new((!self.source_missing).then(|| self.source.clone())),
            })
        }

        async fn mark_delivered(
            &self,
            _: NotificationDeliveryId,
            lease_token: Uuid,
            provider_message_id: &str,
            completed_at: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            self.record_finalization(
                FinalizationCall::Delivered {
                    lease_token,
                    provider_message_id: provider_message_id.to_owned(),
                    completed_at,
                },
                PersistedState::Delivered,
            )
            .await
        }

        async fn mark_retryable_failure(
            &self,
            _: NotificationDeliveryId,
            lease_token: Uuid,
            error_code: &str,
            completed_at: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            self.record_finalization(
                FinalizationCall::RetryableFailure {
                    lease_token,
                    error_code: error_code.to_owned(),
                    completed_at,
                },
                PersistedState::Pending,
            )
            .await
        }

        async fn mark_permanent_failure(
            &self,
            _: NotificationDeliveryId,
            lease_token: Uuid,
            error_code: &str,
            completed_at: OffsetDateTime,
        ) -> Result<bool, NotificationDeliveryError> {
            self.record_finalization(
                FinalizationCall::PermanentFailure {
                    lease_token,
                    error_code: error_code.to_owned(),
                    completed_at,
                },
                PersistedState::Failed,
            )
            .await
        }
    }

    #[derive(Clone, Copy, Default)]
    enum SendOutcome {
        #[default]
        Delivered,
        Retryable(&'static str),
        Permanent(&'static str),
        Ambiguous(&'static str),
        Hang,
    }

    struct RecordingEmailSender {
        sent_sources: Mutex<Vec<NotificationDeliverySource>>,
        outcome: SendOutcome,
        delay: StdDuration,
        cancellations: AtomicUsize,
    }

    impl Default for RecordingEmailSender {
        fn default() -> Self {
            Self {
                sent_sources: Mutex::new(Vec::new()),
                outcome: SendOutcome::default(),
                delay: StdDuration::ZERO,
                cancellations: AtomicUsize::new(0),
            }
        }
    }

    impl RecordingEmailSender {
        fn with_outcome(outcome: SendOutcome) -> Self {
            Self {
                outcome,
                ..Self::default()
            }
        }
    }

    #[async_trait::async_trait]
    impl NotificationChannelSender for RecordingEmailSender {
        fn channel(&self) -> NotificationDeliveryChannel {
            NotificationDeliveryChannel::Email
        }

        async fn send(
            &self,
            source: &NotificationDeliverySource,
        ) -> Result<SentNotificationDelivery, NotificationChannelSendError> {
            self.sent_sources
                .lock()
                .map_err(|_| NotificationChannelSendError::Permanent {
                    code: "TEST_SENDER_LOCK_FAILED",
                    source: box_error(std::io::Error::other("test sender lock poisoned")),
                })?
                .push(source.clone());
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            match self.outcome {
                SendOutcome::Hang => {
                    let _probe = CancellationProbe(&self.cancellations);
                    std::future::pending().await
                }
                SendOutcome::Ambiguous(code) => Err(NotificationChannelSendError::Ambiguous {
                    code,
                    source: box_error(std::io::Error::other("provider accepted but response lost")),
                }),
                SendOutcome::Delivered => Ok(SentNotificationDelivery {
                    provider_message_id: "provider-message-1".to_owned(),
                }),
                SendOutcome::Retryable(code) => Err(NotificationChannelSendError::Retryable {
                    code,
                    source: box_error(std::io::Error::other("test retryable provider failure")),
                }),
                SendOutcome::Permanent(code) => Err(NotificationChannelSendError::Permanent {
                    code,
                    source: box_error(std::io::Error::other("test permanent provider failure")),
                }),
            }
        }
    }

    fn source(notification_delivery_id: NotificationDeliveryId) -> NotificationDeliverySource {
        NotificationDeliverySource {
            notification_delivery_id,
            notification_id: NotificationId::new(),
            user_id: UserId::new(),
            channel: NotificationDeliveryChannel::Email,
            target_key: NotificationDeliveryTargetKey::primary(),
            content: NotificationContent::PartnershipApplication {
                partnership_application_id: PartnershipApplicationId::new(),
                snapshot: PartnershipApplicationNotificationSnapshot {
                    party_name: PartyName::try_from("Test Party")
                        .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
                    listing_source_name: ListingSourceName::try_from("Test Listing Source")
                        .unwrap_or_else(|error| {
                            panic!("invalid test listing source name: {error}")
                        }),
                    image: None,
                },
                decision: PartnershipApplicationDecision::Approved,
            },
            presentation_preferences: crate::presentation::NotificationPresentationPreferences {
                language: Language::En,
                show_unassessed_or_sensitive_content: false,
            },
        }
    }

    fn repository_error() -> NotificationDeliveryError {
        NotificationDeliveryError::OperationFailed {
            source: box_error(std::io::Error::other("test repository lock poisoned")),
        }
    }

    fn claimed_lease_token(
        state: &DeliveryState,
        notification_delivery_id: NotificationDeliveryId,
    ) -> Result<Uuid, Box<dyn std::error::Error>> {
        let claimed_delivery_ids = state
            .claimed_delivery_ids
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .clone();
        assert_eq!(vec![notification_delivery_id], claimed_delivery_ids);
        state
            .claimed_lease_tokens
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .first()
            .copied()
            .ok_or_else(|| std::io::Error::other("missing claimed lease token").into())
    }

    #[tokio::test]
    async fn should_claim_send_and_finalize_through_registered_channel()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let source = source(notification_delivery_id);
        let state = Arc::new(DeliveryState::default());
        *state
            .finalization_failures_remaining
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))? = 1;
        let sender = Arc::new(RecordingEmailSender::default());
        let dispatcher = NotificationDeliveryDispatcher::new(vec![
            sender.clone() as Arc<dyn NotificationChannelSender>
        ])?;
        let handler = DeliverNotificationHandler::new(
            FakeDeliveryRepository::new(notification_delivery_id, source.clone(), state.clone()),
            dispatcher,
        );

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
            })
            .await?;

        assert_eq!(
            DeliverNotificationResult::Delivered { attempt_count: 2 },
            result
        );
        assert_eq!(
            vec![notification_delivery_id],
            state
                .claimed_delivery_ids
                .lock()
                .map_err(|_| std::io::Error::other("test state lock poisoned"))?
                .clone()
        );
        assert_eq!(
            vec![source],
            sender
                .sent_sources
                .lock()
                .map_err(|_| std::io::Error::other("test sender lock poisoned"))?
                .clone()
        );
        assert_eq!(
            vec!["provider-message-1".to_owned()],
            state
                .delivered_message_ids
                .lock()
                .map_err(|_| std::io::Error::other("test state lock poisoned"))?
                .clone()
        );
        let finalization_calls = state
            .finalization_calls
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .clone();
        assert_eq!(2, finalization_calls.len());
        assert!(matches!(
            finalization_calls.as_slice(),
            [
                FinalizationCall::Delivered {
                    lease_token: first_lease_token,
                    provider_message_id: first_provider_message_id,
                    completed_at: first_completed_at,
                },
                FinalizationCall::Delivered {
                    lease_token: second_lease_token,
                    provider_message_id: second_provider_message_id,
                    completed_at: second_completed_at,
                }
            ] if first_lease_token == second_lease_token
                && first_provider_message_id == second_provider_message_id
                && first_completed_at == second_completed_at
                && first_provider_message_id == "provider-message-1"
        ));
        assert_eq!(
            PersistedState::Delivered,
            *state
                .persisted_state
                .lock()
                .map_err(|_| std::io::Error::other("test state lock poisoned"))?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_retry_retryable_failure_finalization_without_sending_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let state = Arc::new(DeliveryState::default());
        *state
            .finalization_failures_remaining
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))? = 1;
        let sender = Arc::new(RecordingEmailSender::with_outcome(SendOutcome::Retryable(
            "TEST_PROVIDER_RETRYABLE",
        )));
        let dispatcher = NotificationDeliveryDispatcher::new(vec![
            sender.clone() as Arc<dyn NotificationChannelSender>
        ])?;
        let handler = DeliverNotificationHandler::new(
            FakeDeliveryRepository::new(
                notification_delivery_id,
                source(notification_delivery_id),
                state.clone(),
            ),
            dispatcher,
        );

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
            })
            .await;

        assert!(matches!(
            result,
            Err(DeliverNotificationError::RetryableSend(error))
                if error.code() == "TEST_PROVIDER_RETRYABLE"
        ));
        let sent_source_count = sender
            .sent_sources
            .lock()
            .map_err(|_| std::io::Error::other("test sender lock poisoned"))?
            .len();
        assert_eq!(1, sent_source_count);
        let finalization_calls = state
            .finalization_calls
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .clone();
        let claimed_lease_token = claimed_lease_token(&state, notification_delivery_id)?;
        assert_eq!(2, finalization_calls.len());
        assert!(matches!(
            finalization_calls.as_slice(),
            [
                FinalizationCall::RetryableFailure {
                    lease_token: first_lease_token,
                    error_code: first_error_code,
                    completed_at: first_completed_at,
                },
                FinalizationCall::RetryableFailure {
                    lease_token: second_lease_token,
                    error_code: second_error_code,
                    completed_at: second_completed_at,
                }
            ] if first_lease_token == second_lease_token
                && *first_lease_token == claimed_lease_token
                && first_error_code == second_error_code
                && first_completed_at == second_completed_at
                && first_error_code == "TEST_PROVIDER_RETRYABLE"
        ));
        let persisted_state = *state
            .persisted_state
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?;
        assert_eq!(PersistedState::Pending, persisted_state);
        Ok(())
    }

    #[tokio::test]
    async fn should_retry_permanent_failure_finalization_without_sending_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let state = Arc::new(DeliveryState::default());
        *state
            .finalization_failures_remaining
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))? = 1;
        let sender = Arc::new(RecordingEmailSender::with_outcome(SendOutcome::Permanent(
            "TEST_PROVIDER_PERMANENT",
        )));
        let dispatcher = NotificationDeliveryDispatcher::new(vec![
            sender.clone() as Arc<dyn NotificationChannelSender>
        ])?;
        let handler = DeliverNotificationHandler::new(
            FakeDeliveryRepository::new(
                notification_delivery_id,
                source(notification_delivery_id),
                state.clone(),
            ),
            dispatcher,
        );

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
            })
            .await?;

        assert_eq!(DeliverNotificationResult::PermanentlyFailed, result);
        let sent_source_count = sender
            .sent_sources
            .lock()
            .map_err(|_| std::io::Error::other("test sender lock poisoned"))?
            .len();
        assert_eq!(1, sent_source_count);
        let finalization_calls = state
            .finalization_calls
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?
            .clone();
        let claimed_lease_token = claimed_lease_token(&state, notification_delivery_id)?;
        assert_eq!(2, finalization_calls.len());
        assert!(matches!(
            finalization_calls.as_slice(),
            [
                FinalizationCall::PermanentFailure {
                    lease_token: first_lease_token,
                    error_code: first_error_code,
                    completed_at: first_completed_at,
                },
                FinalizationCall::PermanentFailure {
                    lease_token: second_lease_token,
                    error_code: second_error_code,
                    completed_at: second_completed_at,
                }
            ] if first_lease_token == second_lease_token
                && *first_lease_token == claimed_lease_token
                && first_error_code == second_error_code
                && first_completed_at == second_completed_at
                && first_error_code == "TEST_PROVIDER_PERMANENT"
        ));
        let persisted_state = *state
            .persisted_state
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))?;
        assert_eq!(PersistedState::Failed, persisted_state);
        Ok(())
    }

    fn state_lock<T>(value: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, std::io::Error> {
        value
            .lock()
            .map_err(|_| std::io::Error::other("test state lock poisoned"))
    }

    fn test_repository() -> (
        DeliverNotificationCommand,
        Arc<DeliveryState>,
        FakeDeliveryRepository,
    ) {
        let command = DeliverNotificationCommand {
            notification_delivery_id: NotificationDeliveryId::new(),
        };
        let state = Arc::new(DeliveryState::default());
        let repository = FakeDeliveryRepository::new(
            command.notification_delivery_id,
            source(command.notification_delivery_id),
            state.clone(),
        );
        (command, state, repository)
    }

    fn test_dispatcher(
        sender: Arc<RecordingEmailSender>,
    ) -> Result<NotificationDeliveryDispatcher, NotificationDeliveryDispatcherRegistrationError>
    {
        NotificationDeliveryDispatcher::new([sender as Arc<dyn NotificationChannelSender>])
    }

    fn short_timing() -> Result<DeliverNotificationTiming, InvalidDeliverNotificationTiming> {
        DeliverNotificationTiming::new(
            StdDuration::from_millis(160),
            StdDuration::from_millis(40),
            StdDuration::from_millis(2),
            StdDuration::from_millis(10),
        )
    }

    #[rstest::rstest]
    #[case(0, 30, 1, 5)]
    #[case(240_001, 30, 1, 5)]
    #[case(100, 0, 1, 5)]
    #[case(100, 100, 1, 5)]
    #[case(100, 20, 0, 5)]
    #[case(100, 20, 6, 5)]
    #[case(100, 20, 1, 100)]
    fn should_reject_unsafe_timing(
        #[case] attempt: u64,
        #[case] operation: u64,
        #[case] initial: u64,
        #[case] max: u64,
    ) {
        assert!(
            DeliverNotificationTiming::new(
                StdDuration::from_millis(attempt),
                StdDuration::from_millis(operation),
                StdDuration::from_millis(initial),
                StdDuration::from_millis(max),
            )
            .is_err()
        );
    }

    #[test]
    fn should_default_to_four_minute_attempt_inside_five_minute_lease() {
        assert_eq!(
            StdDuration::from_secs(240),
            DeliverNotificationTiming::default().attempt_timeout
        );
        assert_eq!(Duration::minutes(5), DELIVERY_LEASE_DURATION);
    }

    #[rstest::rstest]
    #[case(
        ClaimNotificationDeliveryOutcome::Missing,
        DeliverNotificationResult::DeliveryMissing
    )]
    #[case(
        ClaimNotificationDeliveryOutcome::Delivered,
        DeliverNotificationResult::AlreadyDelivered
    )]
    #[case(
        ClaimNotificationDeliveryOutcome::PermanentlyFailed,
        DeliverNotificationResult::PermanentlyFailed
    )]
    #[case(ClaimNotificationDeliveryOutcome::Reclaimable, DeliverNotificationResult::ClaimDeferred { retry_after: StdDuration::from_secs(1) })]
    #[case(ClaimNotificationDeliveryOutcome::AlreadyClaimed { lease_expires_at: OffsetDateTime::UNIX_EPOCH + Duration::seconds(83) }, DeliverNotificationResult::AlreadyClaimed { lease_expires_at: OffsetDateTime::UNIX_EPOCH + Duration::seconds(83) })]
    #[tokio::test]
    async fn should_preserve_claim_outcome_without_sending_or_finalizing(
        #[case] outcome: ClaimNotificationDeliveryOutcome,
        #[case] expected: DeliverNotificationResult,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use crate::ports::notification_delivery_repository::MockNotificationDeliveryRepository;
        let mut repository = MockNotificationDeliveryRepository::new();
        repository
            .expect_claim_and_load_source()
            .times(1)
            .return_once(move |_, now, expiry, _| {
                assert_eq!(DELIVERY_LEASE_DURATION, expiry - now);
                Box::pin(async move { Ok(outcome) })
            });
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::new(repository, test_dispatcher(sender.clone())?);
        assert_eq!(
            expected,
            handler
                .execute(DeliverNotificationCommand {
                    notification_delivery_id: NotificationDeliveryId::new()
                })
                .await?
        );
        assert!(state_lock(&sender.sent_sources)?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_claim_database_source_without_sending()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::ports::notification_delivery_repository::MockNotificationDeliveryRepository;
        let mut repository = MockNotificationDeliveryRepository::new();
        repository
            .expect_claim_and_load_source()
            .times(1)
            .return_once(|_, _, _, _| Box::pin(async { Err(repository_error()) }));
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::new(repository, test_dispatcher(sender.clone())?);
        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id: NotificationDeliveryId::new(),
            })
            .await;
        assert!(
            matches!(result, Err(DeliverNotificationError::Repository(NotificationDeliveryError::OperationFailed { source })) if source.downcast_ref::<std::io::Error>().is_some())
        );
        assert!(state_lock(&sender.sent_sources)?.is_empty());
        Ok(())
    }

    #[rstest::rstest]
    #[case(SendOutcome::Delivered, PersistedState::Delivered)]
    #[case(SendOutcome::Retryable("TEMPORARY"), PersistedState::Pending)]
    #[case(SendOutcome::Permanent("PERMANENT"), PersistedState::Failed)]
    #[tokio::test]
    async fn should_replay_committed_finalization_after_lost_response_without_resending(
        #[case] outcome: SendOutcome,
        #[case] expected_state: PersistedState,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, repository) = test_repository();
        *state_lock(&state.finalization_failures_remaining)? = 2;
        *state_lock(&state.finalization_response_losses_remaining)? = 1;
        let sender = Arc::new(RecordingEmailSender::with_outcome(outcome));
        let handler = DeliverNotificationHandler::with_timing(
            repository,
            test_dispatcher(sender.clone())?,
            short_timing()?,
        );
        let result = handler.execute(command).await;
        match outcome {
            SendOutcome::Retryable(_) => assert!(matches!(
                result,
                Err(DeliverNotificationError::RetryableSend(_))
            )),
            _ => assert!(result.is_ok()),
        }
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert_eq!(expected_state, *state_lock(&state.persisted_state)?);
        let calls = state_lock(&state.finalization_calls)?;
        assert_eq!(4, calls.len());
        assert!(calls.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(
            calls.first(),
            state_lock(&state.persisted_completion)?.as_ref()
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_retry_exact_receipt_when_commit_succeeds_but_response_times_out()
    -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        repository.finalization_behavior = FinalizationBehavior::CommitThenHang;
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::with_timing(
            repository,
            test_dispatcher(sender.clone())?,
            short_timing()?,
        );
        assert_eq!(
            DeliverNotificationResult::Delivered { attempt_count: 2 },
            handler.execute(command).await?
        );
        assert_eq!(1, state.finalization_cancellations.load(Ordering::SeqCst));
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert_eq!(1, state_lock(&state.delivered_message_ids)?.len());
        let calls = state_lock(&state.finalization_calls)?;
        assert_eq!(2, calls.len());
        assert_eq!(calls[0], calls[1]);
        assert_eq!(
            PersistedState::Delivered,
            *state_lock(&state.persisted_state)?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_return_unconfirmed_completion_when_all_committed_responses_are_lost()
    -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, repository) = test_repository();
        *state_lock(&state.finalization_response_losses_remaining)? = usize::MAX;
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::with_timing(
            repository,
            test_dispatcher(sender.clone())?,
            short_timing()?,
        );
        assert!(matches!(
            handler.execute(command).await,
            Err(DeliverNotificationError::FinalizationExhausted { .. })
        ));
        assert_eq!(
            PersistedState::Delivered,
            *state_lock(&state.persisted_state)?
        );
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert_eq!(1, state_lock(&state.delivered_message_ids)?.len());
        let calls = state_lock(&state.finalization_calls)?;
        assert!(calls.len() > 1);
        assert!(calls.windows(2).all(|pair| pair[0] == pair[1]));
        Ok(())
    }

    #[rstest::rstest]
    #[case(false)]
    #[case(true)]
    #[tokio::test]
    async fn should_cap_work_by_actual_claimed_expiry_even_with_larger_attempt_budget(
        #[case] finalize: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        repository.claimed.lease_expires_at =
            OffsetDateTime::now_utc() + Duration::milliseconds(80);
        if finalize {
            repository.finalization_behavior = FinalizationBehavior::Hang;
        }
        let sender = Arc::new(RecordingEmailSender::with_outcome(if finalize {
            SendOutcome::Delivered
        } else {
            SendOutcome::Hang
        }));
        let handler = DeliverNotificationHandler::new(repository, test_dispatcher(sender.clone())?);
        let result =
            tokio::time::timeout(StdDuration::from_secs(2), handler.execute(command)).await?;
        if finalize {
            assert!(matches!(
                result,
                Err(DeliverNotificationError::FinalizationExhausted { .. })
            ));
        } else {
            assert!(matches!(
                result,
                Err(DeliverNotificationError::AttemptTimedOut {
                    phase: DeliveryAttemptPhase::Provider
                })
            ));
        }
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert_eq!(
            PersistedState::Processing,
            *state_lock(&state.persisted_state)?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_finalize_missing_source_before_returning_terminal_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        repository.source_missing = true;
        *state_lock(&state.finalization_failures_remaining)? = 1;
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::with_timing(
            repository,
            test_dispatcher(sender.clone())?,
            short_timing()?,
        );
        assert_eq!(
            DeliverNotificationResult::SourceMissing,
            handler.execute(command).await?
        );
        assert!(state_lock(&sender.sent_sources)?.is_empty());
        assert_eq!(PersistedState::Failed, *state_lock(&state.persisted_state)?);
        assert_eq!(
            vec!["NOTIFICATION_SOURCE_MISSING"],
            *state_lock(&state.permanent_failure_codes)?
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case(FinalizationBehavior::LeaseLost)]
    #[case(FinalizationBehavior::Invalid)]
    #[tokio::test]
    async fn should_stop_finalizing_when_lease_lost_or_persisted_state_invalid(
        #[case] behavior: FinalizationBehavior,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        repository.finalization_behavior = behavior;
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::new(repository, test_dispatcher(sender.clone())?);
        let result = handler.execute(command).await;
        match behavior {
            FinalizationBehavior::LeaseLost => {
                assert!(matches!(result, Err(DeliverNotificationError::LeaseLost)))
            }
            _ => assert!(matches!(
                result,
                Err(DeliverNotificationError::Repository(
                    NotificationDeliveryError::InvalidPersistedState { .. }
                ))
            )),
        }
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert_eq!(1, state_lock(&state.finalization_calls)?.len());
        assert_eq!(
            PersistedState::Processing,
            *state_lock(&state.persisted_state)?
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_not_send_when_claim_returns_expired_lease()
    -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        repository.claimed.lease_expires_at = OffsetDateTime::now_utc() - Duration::seconds(1);
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::new(repository, test_dispatcher(sender.clone())?);
        assert!(matches!(
            handler.execute(command).await,
            Err(DeliverNotificationError::LeaseLost)
        ));
        assert!(state_lock(&sender.sent_sources)?.is_empty());
        assert!(state_lock(&state.finalization_calls)?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_bound_claim_without_sending_when_repository_hangs()
    -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        repository.claim_hangs = true;
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::with_timing(
            repository,
            test_dispatcher(sender.clone())?,
            short_timing()?,
        );
        let result =
            tokio::time::timeout(StdDuration::from_secs(2), handler.execute(command)).await?;
        assert!(matches!(
            result,
            Err(DeliverNotificationError::AttemptTimedOut {
                phase: DeliveryAttemptPhase::Claim
            })
        ));
        assert!(state_lock(&sender.sent_sources)?.is_empty());
        assert!(state_lock(&state.finalization_calls)?.is_empty());
        Ok(())
    }

    #[rstest::rstest]
    #[case(SendOutcome::Hang)]
    #[case(SendOutcome::Ambiguous("RESPONSE_LOST"))]
    #[case(SendOutcome::Ambiguous("SES_SEND_AMBIGUOUS"))]
    #[case(SendOutcome::Ambiguous("SES_MESSAGE_ID_MISSING"))]
    #[tokio::test]
    async fn should_keep_processing_without_finalizing_when_provider_acceptance_is_unknown(
        #[case] outcome: SendOutcome,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, repository) = test_repository();
        let sender = Arc::new(RecordingEmailSender::with_outcome(outcome));
        let handler = DeliverNotificationHandler::with_timing(
            repository,
            test_dispatcher(sender.clone())?,
            short_timing()?,
        );
        let result =
            tokio::time::timeout(StdDuration::from_secs(2), handler.execute(command)).await?;
        match outcome {
            SendOutcome::Hang => {
                assert!(matches!(
                    result,
                    Err(DeliverNotificationError::AttemptTimedOut {
                        phase: DeliveryAttemptPhase::Provider
                    })
                ));
                assert_eq!(1, sender.cancellations.load(Ordering::SeqCst));
            }
            _ => assert!(matches!(
                result,
                Err(DeliverNotificationError::AmbiguousSend(_))
            )),
        }
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert!(state_lock(&state.finalization_calls)?.is_empty());
        assert_eq!(
            PersistedState::Processing,
            *state_lock(&state.persisted_state)?
        );
        Ok(())
    }

    #[rstest::rstest]
    #[case(FinalizationBehavior::Normal)]
    #[case(FinalizationBehavior::Hang)]
    #[tokio::test]
    async fn should_exhaust_finalization_budget_without_resending_or_confirming_delivery(
        #[case] behavior: FinalizationBehavior,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        repository.finalization_behavior = behavior;
        *state_lock(&state.finalization_failures_remaining)? = usize::MAX;
        let sender = Arc::new(RecordingEmailSender::default());
        let handler = DeliverNotificationHandler::with_timing(
            repository,
            test_dispatcher(sender.clone())?,
            short_timing()?,
        );
        let result =
            tokio::time::timeout(StdDuration::from_secs(2), handler.execute(command)).await?;
        assert!(matches!(
            result,
            Err(DeliverNotificationError::FinalizationExhausted {
                source: NotificationDeliveryError::OperationFailed { .. }
            })
        ));
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert_eq!(
            PersistedState::Processing,
            *state_lock(&state.persisted_state)?
        );
        let calls = state_lock(&state.finalization_calls)?;
        assert!(calls.len() >= 2);
        assert!(calls.windows(2).all(|pair| pair[0] == pair[1]));
        if matches!(behavior, FinalizationBehavior::Hang) {
            assert_eq!(
                calls.len(),
                state.finalization_cancellations.load(Ordering::SeqCst)
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_share_one_budget_across_claim_provider_and_finalization()
    -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        repository.claim_delay = StdDuration::from_millis(80);
        repository.finalization_behavior = FinalizationBehavior::Hang;
        let sender = Arc::new(RecordingEmailSender {
            delay: StdDuration::from_millis(80),
            ..Default::default()
        });
        let timing = DeliverNotificationTiming::new(
            StdDuration::from_millis(240),
            StdDuration::from_millis(180),
            StdDuration::from_millis(2),
            StdDuration::from_millis(5),
        )?;
        let handler = DeliverNotificationHandler::with_timing(
            repository,
            test_dispatcher(sender.clone())?,
            timing,
        );
        let started = Instant::now();
        let result =
            tokio::time::timeout(StdDuration::from_secs(2), handler.execute(command)).await?;
        assert!(matches!(
            result,
            Err(DeliverNotificationError::FinalizationExhausted { .. })
        ));
        assert!(started.elapsed() < StdDuration::from_millis(400));
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert_eq!(1, state_lock(&state.finalization_calls)?.len());
        Ok(())
    }

    #[rstest::rstest]
    #[case(false)]
    #[case(true)]
    #[tokio::test]
    async fn should_cancel_in_flight_work_without_finalizing_or_resending_on_shutdown(
        #[case] cancel_finalization: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (command, state, mut repository) = test_repository();
        let sender = Arc::new(RecordingEmailSender::with_outcome(if cancel_finalization {
            SendOutcome::Delivered
        } else {
            SendOutcome::Hang
        }));
        if cancel_finalization {
            repository.finalization_behavior = FinalizationBehavior::Hang;
        }
        let handler = DeliverNotificationHandler::new(repository, test_dispatcher(sender.clone())?);
        let task = tokio::spawn(async move { handler.execute(command).await });
        tokio::time::timeout(StdDuration::from_secs(2), async {
            loop {
                let reached_phase = if cancel_finalization {
                    !state_lock(&state.finalization_calls)?.is_empty()
                } else {
                    !state_lock(&sender.sent_sources)?.is_empty()
                };
                if reached_phase {
                    return Ok::<_, std::io::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert_eq!(1, state_lock(&sender.sent_sources)?.len());
        assert_eq!(
            usize::from(cancel_finalization),
            state_lock(&state.finalization_calls)?.len()
        );
        assert_eq!(
            1,
            if cancel_finalization {
                state.finalization_cancellations.load(Ordering::SeqCst)
            } else {
                sender.cancellations.load(Ordering::SeqCst)
            }
        );
        assert_eq!(
            PersistedState::Processing,
            *state_lock(&state.persisted_state)?
        );
        assert!(state_lock(&state.persisted_completion)?.is_none());
        Ok(())
    }

    #[test]
    fn should_reject_duplicate_channel_registration() {
        let sender = Arc::new(RecordingEmailSender::default());

        let result = NotificationDeliveryDispatcher::new(vec![
            sender.clone() as Arc<dyn NotificationChannelSender>,
            sender as Arc<dyn NotificationChannelSender>,
        ]);

        assert!(matches!(
            result,
            Err(
                NotificationDeliveryDispatcherRegistrationError::DuplicateChannelRegistration {
                    channel: NotificationDeliveryChannel::Email,
                }
            )
        ));
    }

    #[tokio::test]
    async fn should_finalize_and_report_unregistered_channel()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification_delivery_id = NotificationDeliveryId::new();
        let state = Arc::new(DeliveryState::default());
        let handler = DeliverNotificationHandler::new(
            FakeDeliveryRepository::new(
                notification_delivery_id,
                source(notification_delivery_id),
                state.clone(),
            ),
            NotificationDeliveryDispatcher::new(Vec::new())?,
        );

        let result = handler
            .execute(DeliverNotificationCommand {
                notification_delivery_id,
            })
            .await;

        assert!(matches!(
            result,
            Err(DeliverNotificationError::UnregisteredChannel {
                channel: NotificationDeliveryChannel::Email,
            })
        ));
        assert_eq!(
            vec![UNREGISTERED_CHANNEL_ERROR_CODE.to_owned()],
            state
                .permanent_failure_codes
                .lock()
                .map_err(|_| std::io::Error::other("test state lock poisoned"))?
                .clone()
        );
        Ok(())
    }
}

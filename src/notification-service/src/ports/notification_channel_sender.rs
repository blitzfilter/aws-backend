use crate::ports::notification_delivery_repository::NotificationDeliverySource;
use application::error::BoxError;
use notification_core::notification_delivery::NotificationDeliveryChannel;
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentNotificationDelivery {
    pub provider_message_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationChannelSendError {
    #[error("notification channel send failed temporarily: {code}")]
    Retryable {
        code: &'static str,
        #[source]
        source: BoxError,
    },
    /// Acceptance is unknown; keep the lease and do not send again in this attempt.
    #[error("notification channel send outcome is unknown: {code}")]
    Ambiguous {
        code: &'static str,
        #[source]
        source: BoxError,
    },
    #[error("notification channel send failed permanently: {code}")]
    Permanent {
        code: &'static str,
        #[source]
        source: BoxError,
    },
}

impl NotificationChannelSendError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Retryable { code, .. }
            | Self::Ambiguous { code, .. }
            | Self::Permanent { code, .. } => code,
        }
    }
}

#[async_trait::async_trait]
pub trait NotificationChannelSender: Send + Sync {
    fn channel(&self) -> NotificationDeliveryChannel;

    /// Called at most once per service attempt. Cancellation/timeout may follow provider
    /// acceptance; implementations must not detach work or claim exactly-once delivery.
    async fn send(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<SentNotificationDelivery, NotificationChannelSendError>;
}

pub struct NotificationDeliveryDispatcher {
    senders: HashMap<NotificationDeliveryChannel, Arc<dyn NotificationChannelSender>>,
}

impl NotificationDeliveryDispatcher {
    pub fn new(
        senders: impl IntoIterator<Item = Arc<dyn NotificationChannelSender>>,
    ) -> Result<Self, NotificationDeliveryDispatcherRegistrationError> {
        let mut dispatcher = Self {
            senders: HashMap::new(),
        };
        for sender in senders {
            dispatcher.register(sender)?;
        }
        Ok(dispatcher)
    }

    pub fn register(
        &mut self,
        sender: Arc<dyn NotificationChannelSender>,
    ) -> Result<(), NotificationDeliveryDispatcherRegistrationError> {
        let channel = sender.channel();
        if self.senders.contains_key(&channel) {
            return Err(
                NotificationDeliveryDispatcherRegistrationError::DuplicateChannelRegistration {
                    channel,
                },
            );
        }
        self.senders.insert(channel, sender);
        Ok(())
    }

    pub fn validate_channels(
        &self,
        channels: impl IntoIterator<Item = NotificationDeliveryChannel>,
    ) -> Result<(), NotificationDeliveryDispatchError> {
        for channel in channels {
            if !self.senders.contains_key(&channel) {
                return Err(NotificationDeliveryDispatchError::UnregisteredChannel { channel });
            }
        }
        Ok(())
    }

    pub async fn dispatch(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<SentNotificationDelivery, NotificationDeliveryDispatchError> {
        let sender = self.senders.get(&source.channel).ok_or(
            NotificationDeliveryDispatchError::UnregisteredChannel {
                channel: source.channel,
            },
        )?;
        sender
            .send(source)
            .await
            .map_err(NotificationDeliveryDispatchError::Send)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationDeliveryDispatcherRegistrationError {
    #[error("notification channel sender already registered for {channel:?}")]
    DuplicateChannelRegistration {
        channel: NotificationDeliveryChannel,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum NotificationDeliveryDispatchError {
    #[error("notification channel sender is not registered for {channel:?}")]
    UnregisteredChannel {
        channel: NotificationDeliveryChannel,
    },
    #[error("notification channel send failed")]
    Send(#[source] NotificationChannelSendError),
}

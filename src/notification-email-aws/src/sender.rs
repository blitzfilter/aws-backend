use crate::{
    provider_failure::SesSendFailed,
    template_mapping::{
        EmailLanguage, ses_template_tag_value, subject, template_data, template_type,
    },
    template_reader::TemplateReader,
};
use application::error::box_error;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::{
    Client as SesClient,
    config::retry::RetryConfig,
    operation::send_email::SendEmailOutput,
    types::{Body, Content, Destination, EmailContent, Message, MessageTag},
};
use notification_core::notification_delivery::NotificationDeliveryChannel;
use notification_email::{EmailDeliveryTargetReadError, EmailDeliveryTargetReader};
use notification_service::ports::{
    notification_channel_sender::{
        NotificationChannelSendError, NotificationChannelSender, SentNotificationDelivery,
    },
    notification_delivery_repository::NotificationDeliverySource,
};
use std::sync::Arc;

pub struct EmailDeliveryConfig {
    template_bucket: String,
    from_email_address: String,
    reply_to_email_address: String,
    stage: String,
    commit_sha: String,
}

impl EmailDeliveryConfig {
    pub fn new(
        template_bucket: impl Into<String>,
        from_email_address: impl Into<String>,
        reply_to_email_address: impl Into<String>,
        stage: impl Into<String>,
        commit_sha: impl Into<String>,
    ) -> Self {
        Self {
            template_bucket: template_bucket.into(),
            from_email_address: from_email_address.into(),
            reply_to_email_address: reply_to_email_address.into(),
            stage: stage.into(),
            commit_sha: commit_sha.into(),
        }
    }
}

pub struct SesNotificationChannelSender {
    ses: SesClient,
    from_email_address: String,
    reply_to_email_address: String,
    templates: TemplateReader,
    targets: Arc<dyn EmailDeliveryTargetReader>,
}

impl SesNotificationChannelSender {
    pub fn new(
        s3: S3Client,
        ses: SesClient,
        config: EmailDeliveryConfig,
        targets: Arc<dyn EmailDeliveryTargetReader>,
    ) -> Self {
        // SendEmail has no idempotency token. SDK retries can send again after an
        // accepted request loses its response, before service sees the ambiguity.
        let ses = SesClient::from_conf(
            ses.config()
                .to_builder()
                .retry_config(RetryConfig::disabled())
                .build(),
        );
        Self {
            ses,
            from_email_address: config.from_email_address,
            reply_to_email_address: config.reply_to_email_address,
            templates: TemplateReader::new(
                s3,
                config.template_bucket,
                config.stage,
                config.commit_sha,
            ),
            targets,
        }
    }

    async fn render(
        &self,
        source: &NotificationDeliverySource,
        first_name: Option<&str>,
    ) -> Result<(String, String, &'static str), NotificationChannelSendError> {
        let template_type = template_type(&source.content);
        let email_language = EmailLanguage::resolve(source.presentation_preferences.language);
        let body = self
            .templates
            .render(
                template_type,
                email_language,
                &template_data(source, email_language, first_name),
            )
            .await?;
        Ok((
            subject(template_type, email_language).to_owned(),
            body,
            ses_template_tag_value(template_type),
        ))
    }
}

#[async_trait::async_trait]
impl NotificationChannelSender for SesNotificationChannelSender {
    fn channel(&self) -> NotificationDeliveryChannel {
        NotificationDeliveryChannel::Email
    }

    async fn send(
        &self,
        source: &NotificationDeliverySource,
    ) -> Result<SentNotificationDelivery, NotificationChannelSendError> {
        let target = self
            .targets
            .find_email_target(source.user_id, &source.target_key)
            .await
            .map_err(target_error)?
            .ok_or_else(|| NotificationChannelSendError::Permanent {
                code: "EMAIL_TARGET_MISSING",
                source: box_error(std::io::Error::other("email delivery target is missing")),
            })?;
        let (subject, body, template_tag_value) =
            self.render(source, target.first_name.as_deref()).await?;
        let message = Message::builder()
            .subject(Content::builder().data(subject).build().map_err(|source| {
                NotificationChannelSendError::Permanent {
                    code: "EMAIL_CONTENT_INVALID",
                    source: box_error(source),
                }
            })?)
            .body(
                Body::builder()
                    .html(Content::builder().data(body).build().map_err(|source| {
                        NotificationChannelSendError::Permanent {
                            code: "EMAIL_CONTENT_INVALID",
                            source: box_error(source),
                        }
                    })?)
                    .build(),
            )
            .build();
        let email_tag = MessageTag::builder()
            .name("template_type")
            .value(template_tag_value)
            .build()
            .map_err(|source| NotificationChannelSendError::Permanent {
                code: "EMAIL_TAG_INVALID",
                source: box_error(source),
            })?;
        let response = self
            .ses
            .send_email()
            .from_email_address(&self.from_email_address)
            .reply_to_addresses(&self.reply_to_email_address)
            .destination(
                Destination::builder()
                    .to_addresses(target.address.to_string())
                    .build(),
            )
            .content(EmailContent::builder().simple(message).build())
            .email_tags(email_tag)
            .send()
            .await
            .map_err(SesSendFailed)?;
        sent_delivery(response)
    }
}

fn sent_delivery(
    response: SendEmailOutput,
) -> Result<SentNotificationDelivery, NotificationChannelSendError> {
    let provider_message_id = response
        .message_id()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| NotificationChannelSendError::Ambiguous {
            code: "SES_MESSAGE_ID_MISSING",
            source: box_error(std::io::Error::other(
                "SES response did not include a nonempty message ID",
            )),
        })?;
    Ok(SentNotificationDelivery {
        provider_message_id: provider_message_id.to_owned(),
    })
}

fn target_error(error: EmailDeliveryTargetReadError) -> NotificationChannelSendError {
    match error {
        EmailDeliveryTargetReadError::ReadFailed { source } => {
            NotificationChannelSendError::Retryable {
                code: "EMAIL_TARGET_READ_FAILED",
                source,
            }
        }
        EmailDeliveryTargetReadError::InvalidPersistedState { source } => {
            NotificationChannelSendError::Permanent {
                code: "EMAIL_TARGET_INVALID",
                source,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notification_core::notification_delivery::NotificationDeliveryTargetKey;
    use notification_email::EmailDeliveryTarget;
    use rstest::rstest;
    use user_core::user_id::UserId;

    struct NoEmailTarget;

    #[async_trait::async_trait]
    impl EmailDeliveryTargetReader for NoEmailTarget {
        async fn find_email_target(
            &self,
            _: UserId,
            _: &NotificationDeliveryTargetKey,
        ) -> Result<Option<EmailDeliveryTarget>, EmailDeliveryTargetReadError> {
            Ok(None)
        }
    }

    #[rstest]
    #[case(1)]
    #[case(3)]
    #[case(7)]
    fn should_disable_ses_sdk_resends_even_when_injected_client_retries(#[case] attempts: u32) {
        let ses = SesClient::from_conf(
            aws_sdk_sesv2::Config::builder()
                .behavior_version_latest()
                .region(aws_sdk_sesv2::config::Region::new("eu-west-1"))
                .retry_config(RetryConfig::standard().with_max_attempts(attempts))
                .build(),
        );
        let s3 = S3Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version_latest()
                .region(aws_sdk_s3::config::Region::new("eu-west-1"))
                .build(),
        );
        let sender = SesNotificationChannelSender::new(
            s3,
            ses.clone(),
            EmailDeliveryConfig::new(
                "test-templates",
                "from@example.test",
                "reply@example.test",
                "test",
                "test-commit",
            ),
            Arc::new(NoEmailTarget),
        );
        assert_eq!(
            Some(1),
            sender
                .ses
                .config()
                .retry_config()
                .map(RetryConfig::max_attempts)
        );
        assert_eq!(
            Some(attempts),
            ses.config().retry_config().map(RetryConfig::max_attempts)
        );
        assert_eq!(ses.config().region(), sender.ses.config().region());
    }

    #[rstest]
    #[case(None)]
    #[case(Some(""))]
    #[case(Some(" \t\n"))]
    fn should_report_ambiguous_acceptance_when_success_response_has_no_usable_receipt(
        #[case] receipt: Option<&str>,
    ) {
        let result = sent_delivery(
            SendEmailOutput::builder()
                .set_message_id(receipt.map(str::to_owned))
                .build(),
        );
        assert!(
            matches!(result, Err(NotificationChannelSendError::Ambiguous { code: "SES_MESSAGE_ID_MISSING", source }) if source.downcast_ref::<std::io::Error>().is_some())
        );
    }

    #[test]
    fn should_preserve_original_provider_receipt_on_success()
    -> Result<(), NotificationChannelSendError> {
        let receipt = "provider-receipt-123";
        assert_eq!(
            SentNotificationDelivery {
                provider_message_id: receipt.to_owned()
            },
            sent_delivery(SendEmailOutput::builder().message_id(receipt).build())?
        );
        Ok(())
    }

    #[test]
    fn should_keep_pre_send_target_lookup_failure_retryable_with_safe_source() {
        let error = target_error(EmailDeliveryTargetReadError::ReadFailed {
            source: box_error(std::io::Error::other("private lookup payload")),
        });
        assert_eq!(
            "notification channel send failed temporarily: EMAIL_TARGET_READ_FAILED",
            error.to_string()
        );
        assert!(
            matches!(error, NotificationChannelSendError::Retryable { source, .. } if source.downcast_ref::<std::io::Error>().is_some())
        );
    }
}

use application::error::{BoxError, box_error};
use aws_sdk_sesv2::{error::SdkError, operation::send_email::SendEmailError};
use notification_service::ports::notification_channel_sender::NotificationChannelSendError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderFailure {
    Retryable { code: &'static str },
    Ambiguous { code: &'static str },
    Permanent { code: &'static str },
}

impl ProviderFailure {
    pub(crate) fn into_send_error(
        self,
        source: impl Into<BoxError>,
    ) -> NotificationChannelSendError {
        match self {
            Self::Retryable { code } => NotificationChannelSendError::Retryable {
                code,
                source: source.into(),
            },
            Self::Ambiguous { code } => NotificationChannelSendError::Ambiguous {
                code,
                source: source.into(),
            },
            Self::Permanent { code } => NotificationChannelSendError::Permanent {
                code,
                source: source.into(),
            },
        }
    }
}

pub(crate) fn classify_s3_template_fetch(
    template_missing: bool,
    status_code: Option<u16>,
) -> ProviderFailure {
    if template_missing || matches!(status_code, Some(404)) {
        ProviderFailure::Permanent {
            code: "S3_TEMPLATE_MISSING",
        }
    } else if matches!(status_code, Some(400..=499) if status_code != Some(408) && status_code != Some(429))
    {
        ProviderFailure::Permanent {
            code: "S3_TEMPLATE_ACCESS_OR_CONFIG_INVALID",
        }
    } else {
        ProviderFailure::Retryable {
            code: "S3_TEMPLATE_FETCH_RETRYABLE",
        }
    }
}

pub(crate) fn classify_ses_send(
    throttled: bool,
    permanently_rejected: bool,
    status_code: Option<u16>,
) -> ProviderFailure {
    if throttled || matches!(status_code, Some(429)) {
        ProviderFailure::Retryable {
            code: "SES_THROTTLED",
        }
    } else if permanently_rejected
        || matches!(status_code, Some(400..=499) if status_code != Some(408))
    {
        ProviderFailure::Permanent {
            code: "SES_REQUEST_OR_CONFIGURATION_INVALID",
        }
    } else {
        // Timeout/5xx does not prove rejection: acceptance may precede response loss.
        ProviderFailure::Ambiguous {
            code: "SES_SEND_AMBIGUOUS",
        }
    }
}

pub(crate) struct SesSendFailed(pub(crate) SdkError<SendEmailError>);

impl From<SesSendFailed> for NotificationChannelSendError {
    fn from(error: SesSendFailed) -> Self {
        let failure = match &error.0 {
            SdkError::ConstructionFailure(_) => ProviderFailure::Permanent {
                code: "SES_REQUEST_BUILD_FAILED",
            },
            SdkError::ServiceError(response) => {
                let service = response.err();
                classify_ses_send(
                    service.is_limit_exceeded_exception()
                        || service.is_too_many_requests_exception(),
                    service.is_bad_request_exception()
                        || service.is_message_rejected()
                        || service.is_account_suspended_exception()
                        || service.is_mail_from_domain_not_verified_exception()
                        || service.is_not_found_exception()
                        || service.is_sending_paused_exception(),
                    Some(response.raw().status().as_u16()),
                )
            }
            // Dispatch may have sent bytes; malformed responses and timeouts give no
            // authoritative acceptance decision. Unknown SDK variants fail closed too.
            _ => ProviderFailure::Ambiguous {
                code: "SES_SEND_AMBIGUOUS",
            },
        };
        failure.into_send_error(box_error(error.0))
    }
}

pub(crate) fn provider_error(
    failure: ProviderFailure,
    source: impl Into<BoxError>,
) -> NotificationChannelSendError {
    failure.into_send_error(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::box_error;
    use rstest::rstest;

    #[rstest]
    #[case(true, None, ProviderFailure::Permanent { code: "S3_TEMPLATE_MISSING" })]
    #[case(false, Some(403), ProviderFailure::Permanent { code: "S3_TEMPLATE_ACCESS_OR_CONFIG_INVALID" })]
    #[case(false, Some(408), ProviderFailure::Retryable { code: "S3_TEMPLATE_FETCH_RETRYABLE" })]
    #[case(false, Some(500), ProviderFailure::Retryable { code: "S3_TEMPLATE_FETCH_RETRYABLE" })]
    fn should_classify_s3_template_fetch_failures(
        #[case] template_missing: bool,
        #[case] status_code: Option<u16>,
        #[case] expected: ProviderFailure,
    ) {
        assert_eq!(
            expected,
            classify_s3_template_fetch(template_missing, status_code)
        );
    }

    #[rstest]
    #[case(true, false, None, ProviderFailure::Retryable { code: "SES_THROTTLED" })]
    #[case(false, true, None, ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(false, false, Some(400), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(false, false, Some(403), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(false, false, Some(429), ProviderFailure::Retryable { code: "SES_THROTTLED" })]
    #[case(false, false, Some(408), ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(false, false, Some(500), ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(false, false, Some(502), ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(false, false, Some(503), ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(false, false, Some(504), ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(false, false, None, ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(false, false, Some(200), ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(false, false, Some(302), ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    fn should_classify_ses_send_failures(
        #[case] throttled: bool,
        #[case] permanently_rejected: bool,
        #[case] status_code: Option<u16>,
        #[case] expected: ProviderFailure,
    ) {
        assert_eq!(
            expected,
            classify_ses_send(throttled, permanently_rejected, status_code)
        );
    }

    fn raw_response(status: u16) -> Result<aws_sdk_sesv2::config::http::HttpResponse, BoxError> {
        Ok(aws_sdk_sesv2::config::http::HttpResponse::new(
            status.try_into()?,
            "private provider payload".into(),
        ))
    }

    fn assert_sdk_failure(source: SdkError<SendEmailError>, expected: ProviderFailure) {
        let error = NotificationChannelSendError::from(SesSendFailed(source));
        assert!(!error.to_string().contains("private provider payload"));
        let (actual, source) = match error {
            NotificationChannelSendError::Retryable { code, source } => {
                (ProviderFailure::Retryable { code }, source)
            }
            NotificationChannelSendError::Ambiguous { code, source } => {
                (ProviderFailure::Ambiguous { code }, source)
            }
            NotificationChannelSendError::Permanent { code, source } => {
                (ProviderFailure::Permanent { code }, source)
            }
        };
        assert_eq!(expected, actual);
        assert!(source.downcast_ref::<SdkError<SendEmailError>>().is_some());
    }

    #[rstest]
    #[case(SendEmailError::TooManyRequestsException(aws_sdk_sesv2::types::error::TooManyRequestsException::builder().build()), ProviderFailure::Retryable { code: "SES_THROTTLED" })]
    #[case(SendEmailError::LimitExceededException(aws_sdk_sesv2::types::error::LimitExceededException::builder().build()), ProviderFailure::Retryable { code: "SES_THROTTLED" })]
    #[case(SendEmailError::BadRequestException(aws_sdk_sesv2::types::error::BadRequestException::builder().build()), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(SendEmailError::MessageRejected(aws_sdk_sesv2::types::error::MessageRejected::builder().build()), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(SendEmailError::AccountSuspendedException(aws_sdk_sesv2::types::error::AccountSuspendedException::builder().build()), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(SendEmailError::MailFromDomainNotVerifiedException(aws_sdk_sesv2::types::error::MailFromDomainNotVerifiedException::builder().build()), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(SendEmailError::NotFoundException(aws_sdk_sesv2::types::error::NotFoundException::builder().build()), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(SendEmailError::SendingPausedException(aws_sdk_sesv2::types::error::SendingPausedException::builder().build()), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    fn should_classify_modeled_ses_rejections_and_retain_sdk_source(
        #[case] service_error: SendEmailError,
        #[case] expected: ProviderFailure,
    ) -> Result<(), BoxError> {
        assert_sdk_failure(
            SdkError::service_error(service_error, raw_response(400)?),
            expected,
        );
        Ok(())
    }

    #[rstest]
    #[case(408, ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(503, ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    #[case(429, ProviderFailure::Retryable { code: "SES_THROTTLED" })]
    #[case(403, ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    fn should_classify_unmodeled_service_response_without_losing_sdk_source(
        #[case] status: u16,
        #[case] expected: ProviderFailure,
    ) -> Result<(), BoxError> {
        assert_sdk_failure(
            SdkError::service_error(
                SendEmailError::unhandled(std::io::Error::other("private provider payload")),
                raw_response(status)?,
            ),
            expected,
        );
        Ok(())
    }

    #[test]
    fn should_keep_timeout_and_dispatch_failure_ambiguous() {
        for source in [
            SdkError::timeout_error(std::io::Error::other("private provider payload")),
            SdkError::dispatch_failure(aws_sdk_sesv2::error::ConnectorError::io(box_error(
                std::io::Error::other("private provider payload"),
            ))),
        ] {
            assert_sdk_failure(
                source,
                ProviderFailure::Ambiguous {
                    code: "SES_SEND_AMBIGUOUS",
                },
            );
        }
    }

    #[rstest]
    #[case(200)]
    #[case(403)]
    #[case(503)]
    fn should_keep_unparseable_response_ambiguous(#[case] status: u16) -> Result<(), BoxError> {
        assert_sdk_failure(
            SdkError::response_error(
                std::io::Error::other("private provider payload"),
                raw_response(status)?,
            ),
            ProviderFailure::Ambiguous {
                code: "SES_SEND_AMBIGUOUS",
            },
        );
        Ok(())
    }

    #[test]
    fn should_classify_request_construction_failure_as_unsent_permanent_failure() {
        assert_sdk_failure(
            SdkError::construction_failure(std::io::Error::other("private provider payload")),
            ProviderFailure::Permanent {
                code: "SES_REQUEST_BUILD_FAILED",
            },
        );
    }

    #[rstest]
    #[case(ProviderFailure::Retryable { code: "SES_THROTTLED" })]
    #[case(ProviderFailure::Ambiguous { code: "SES_SEND_AMBIGUOUS" })]
    fn should_retain_nonterminal_cause_without_exposing_provider_payload(
        #[case] failure: ProviderFailure,
    ) -> Result<(), BoxError> {
        let error = failure.into_send_error(box_error(std::io::Error::other(
            "recipient@example.test private provider payload",
        )));
        assert!(!error.to_string().contains("recipient@example.test"));
        assert!(!error.to_string().contains("private provider payload"));
        let source = std::error::Error::source(&error)
            .ok_or_else(|| std::io::Error::other("missing provider error source"))?;
        assert_eq!(
            Some(std::io::ErrorKind::Other),
            source
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind)
        );
        Ok(())
    }

    #[test]
    fn should_not_include_provider_payload_in_error_display() {
        let error = provider_error(
            ProviderFailure::Permanent {
                code: "SES_REQUEST_OR_CONFIGURATION_INVALID",
            },
            box_error(std::io::Error::other(
                "recipient@example.test provider payload",
            )),
        );

        assert_eq!(
            "notification channel send failed permanently: SES_REQUEST_OR_CONFIGURATION_INVALID",
            error.to_string()
        );
        assert!(!error.to_string().contains("recipient@example.test"));
    }
}

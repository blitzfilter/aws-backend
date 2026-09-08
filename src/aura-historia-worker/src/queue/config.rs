#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

use crate::WorkerScope;
use aws_sdk_sqs::types::QueueAttributeName;
use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use url::Url;

pub const WORKER_QUEUE_URL_ENV: &str = "AURA_HISTORIA_WORKER_QUEUE_URL";
pub const AWS_REGION_ENV: &str = "AWS_REGION";
pub const SQS_ENDPOINT_ENV: &str = "AWS_ENDPOINT_URL_SQS";

/// One Standard queue per deployed scope. Construction performs no AWS mutations.
#[derive(Clone, Debug)]
pub struct SqsQueueConfig {
    pub(super) scope: WorkerScope,
    pub(super) queue_url: Url,
    pub(super) region: String,
    pub(super) stage: String,
    pub(super) endpoint: Option<Url>,
    account: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    #[error("missing worker queue configuration: {0}")]
    MissingConfig(&'static str),
    #[error("invalid worker queue configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("worker queue attribute contract mismatch: {0}")]
    Attribute(&'static str),
    #[error("SQS operation unavailable")]
    Unavailable,
    #[error("SQS operation timed out")]
    Timeout,
    #[error("invalid SQS response metadata")]
    InvalidResponse,
    #[error("worker SQS message exceeds encoded size limit")]
    MessageTooLarge,
    #[error("worker consumer scope mismatch")]
    Scope,
}

impl SqsQueueConfig {
    pub fn new(
        scope: WorkerScope,
        queue_url: Url,
        region: String,
        stage: String,
        local_endpoint: Option<Url>,
    ) -> Result<Self, QueueError> {
        if !valid_label(&stage) || format!("aura-worker-{}-dlq-{stage}", scope.as_str()).len() > 80
        {
            return Err(QueueError::InvalidConfig("STAGE"));
        }
        if !valid_label(&region) {
            return Err(QueueError::InvalidConfig(AWS_REGION_ENV));
        }
        let local = matches!(stage.as_str(), "ephemeral" | "local" | "test");
        if local_endpoint.is_some() && !local {
            return Err(QueueError::InvalidConfig(SQS_ENDPOINT_ENV));
        }
        clean_url(&queue_url)?;
        let segments: Vec<_> = queue_url.path().split('/').collect();
        if segments.len() != 3
            || segments[2] != format!("aura-worker-{}-{stage}", scope.as_str())
            || segments[1].len() != 12
            || !segments[1].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(QueueError::InvalidConfig(WORKER_QUEUE_URL_ENV));
        }
        let account = segments[1].to_owned();
        let aws_host = format!("sqs.{region}.amazonaws.com");
        if let Some(endpoint) = &local_endpoint {
            clean_url(endpoint)?;
            if endpoint.path() != "/"
                || endpoint.origin() != queue_url.origin()
                || !matches!(endpoint.scheme(), "http" | "https")
            {
                return Err(QueueError::InvalidConfig(SQS_ENDPOINT_ENV));
            }
        } else if queue_url.scheme() != "https"
            || queue_url.host_str() != Some(aws_host.as_str())
            || queue_url.port().is_some()
        {
            return Err(QueueError::InvalidConfig(WORKER_QUEUE_URL_ENV));
        }
        Ok(Self {
            scope,
            queue_url,
            region,
            stage,
            endpoint: local_endpoint,
            account,
        })
    }

    pub fn from_env(scope: WorkerScope) -> Result<Self, QueueError> {
        Self::from_getter(scope, |name| std::env::var(name).ok())
    }

    pub(crate) fn from_getter<F>(scope: WorkerScope, mut get: F) -> Result<Self, QueueError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let mut required = |name| {
            get(name)
                .filter(|v| !v.is_empty())
                .ok_or(QueueError::MissingConfig(name))
        };
        let queue_url = required(WORKER_QUEUE_URL_ENV)?
            .parse()
            .map_err(|_| QueueError::InvalidConfig(WORKER_QUEUE_URL_ENV))?;
        let region = required(AWS_REGION_ENV)?;
        let stage = required("STAGE")?;
        // Do not let the AWS global endpoint override bypass the explicit local boundary.
        if get("AWS_ENDPOINT_URL").is_some() {
            return Err(QueueError::InvalidConfig(
                "AWS_ENDPOINT_URL (use AWS_ENDPOINT_URL_SQS)",
            ));
        }
        let endpoint = get(SQS_ENDPOINT_ENV)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| QueueError::InvalidConfig(SQS_ENDPOINT_ENV))
            })
            .transpose()?;
        Self::new(scope, queue_url, region, stage, endpoint)
    }

    pub const fn scope(&self) -> WorkerScope {
        self.scope
    }
    pub fn queue_url(&self) -> &Url {
        &self.queue_url
    }
    pub fn region(&self) -> &str {
        &self.region
    }
    pub fn stage(&self) -> &str {
        &self.stage
    }
    pub fn visibility_timeout(&self) -> Duration {
        visibility(self.scope)
    }
    pub fn execution_budget(&self) -> Duration {
        execution_budget(self.scope)
    }

    pub(super) fn arn(&self, dlq: bool) -> String {
        format!(
            "arn:aws:sqs:{}:{}:{}",
            self.region,
            self.account,
            self.name(dlq)
        )
    }
    fn name(&self, dlq: bool) -> String {
        format!(
            "aura-worker-{}{}-{}",
            self.scope.as_str(),
            if dlq { "-dlq" } else { "" },
            self.stage
        )
    }
    pub(super) fn dlq_url(&self) -> Url {
        let mut url = self.queue_url.clone();
        url.set_path(&format!("/{}/{}", self.account, self.name(true)));
        url
    }
    pub(super) fn endpoint_url(&self) -> String {
        self.endpoint
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("https://sqs.{}.amazonaws.com", self.region))
    }
}

fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn clean_url(url: &Url) -> Result<(), QueueError> {
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(QueueError::InvalidConfig("queue URL"));
    }
    Ok(())
}

pub(super) fn visibility(scope: WorkerScope) -> Duration {
    Duration::from_secs(match scope {
        WorkerScope::NotificationDelivery => 360,
        WorkerScope::SearchFilterPercolator
        | WorkerScope::ProductListingTranslation
        | WorkerScope::ProductListingEmbedding
        | WorkerScope::ProductListingRawNormalization => 300,
        WorkerScope::SearchFilterProjection
        | WorkerScope::SearchFilterMatchNotification
        | WorkerScope::WatchlistNotification
        | WorkerScope::ProductListingContentAssessment
        | WorkerScope::ProductListingOpenSearch => 60,
    })
}
pub(super) fn execution_budget(scope: WorkerScope) -> Duration {
    // Notification's service-owned lease lasts five minutes. Leave finalization/drain headroom.
    Duration::from_secs(if visibility(scope).as_secs() >= 300 {
        240
    } else {
        45
    })
}

pub(super) type Attributes = HashMap<QueueAttributeName, String>;
fn attr(attributes: &Attributes, name: QueueAttributeName) -> Option<&str> {
    attributes.get(&name).map(String::as_str)
}
fn require(
    attributes: &Attributes,
    name: QueueAttributeName,
    expected: &str,
    field: &'static str,
) -> Result<(), QueueError> {
    if attr(attributes, name) != Some(expected) {
        return Err(QueueError::Attribute(field));
    }
    Ok(())
}

pub(super) fn validate_attributes(
    config: &SqsQueueConfig,
    attributes: &Attributes,
    dlq: bool,
) -> Result<(), QueueError> {
    use QueueAttributeName as A;
    let arn = config.arn(dlq);
    require(attributes, A::QueueArn, &arn, "QueueArn")?;
    // AWS omits FifoQueue on Standard queues. Only absent or literal false means Standard.
    if !matches!(attr(attributes, A::FifoQueue), None | Some("false")) {
        return Err(QueueError::Attribute("FifoQueue"));
    }
    require(
        attributes,
        A::MessageRetentionPeriod,
        if dlq { "1209600" } else { "604800" },
        "MessageRetentionPeriod",
    )?;
    if attr(attributes, A::SqsManagedSseEnabled) != Some("true")
        && !attr(attributes, A::KmsMasterKeyId).is_some_and(|key| !key.is_empty())
    {
        return Err(QueueError::Attribute("encryption"));
    }
    let policy: Value =
        serde_json::from_str(attr(attributes, A::Policy).ok_or(QueueError::Attribute("Policy"))?)
            .map_err(|_| QueueError::Attribute("Policy"))?;
    if !tls_denied(&policy, &arn) {
        return Err(QueueError::Attribute("TLS policy"));
    }
    if !private_policy(&policy) {
        return Err(QueueError::Attribute("public access policy"));
    }
    let redrive_allow: Value = serde_json::from_str(
        attr(attributes, A::RedriveAllowPolicy)
            .ok_or(QueueError::Attribute("RedriveAllowPolicy"))?,
    )
    .map_err(|_| QueueError::Attribute("RedriveAllowPolicy"))?;
    let expected_allow = if dlq {
        serde_json::json!({"redrivePermission": "byQueue", "sourceQueueArns": [config.arn(false)]})
    } else {
        serde_json::json!({"redrivePermission": "denyAll"})
    };
    if redrive_allow != expected_allow {
        return Err(QueueError::Attribute("RedriveAllowPolicy"));
    }
    if dlq && attr(attributes, A::RedrivePolicy).is_some() {
        return Err(QueueError::Attribute("DLQ RedrivePolicy"));
    }
    if !dlq {
        validate_source_attributes(config, attributes)?;
    }
    Ok(())
}

fn validate_source_attributes(
    config: &SqsQueueConfig,
    attributes: &Attributes,
) -> Result<(), QueueError> {
    use QueueAttributeName as A;
    require(
        attributes,
        A::VisibilityTimeout,
        &config.visibility_timeout().as_secs().to_string(),
        "VisibilityTimeout",
    )?;
    require(
        attributes,
        A::ReceiveMessageWaitTimeSeconds,
        "20",
        "ReceiveMessageWaitTimeSeconds",
    )?;
    let redrive: Value = serde_json::from_str(
        attr(attributes, A::RedrivePolicy).ok_or(QueueError::Attribute("RedrivePolicy"))?,
    )
    .map_err(|_| QueueError::Attribute("RedrivePolicy"))?;
    if redrive["deadLetterTargetArn"] != config.arn(true)
        || !(redrive["maxReceiveCount"] == 5 || redrive["maxReceiveCount"] == "5")
    {
        return Err(QueueError::Attribute("RedrivePolicy"));
    }
    Ok(())
}

fn contains(value: &Value, expected: &str) -> bool {
    value.as_str() == Some(expected)
        || value
            .as_array()
            .is_some_and(|values| values.iter().any(|v| v.as_str() == Some(expected)))
}
fn statements(policy: &Value) -> &[Value] {
    match &policy["Statement"] {
        Value::Array(statements) => statements.as_slice(),
        Value::Object(_) => std::slice::from_ref(&policy["Statement"]),
        _ => &[],
    }
}

fn private_policy(policy: &Value) -> bool {
    statements(policy)
        .iter()
        .all(|statement| match statement["Effect"].as_str() {
            Some("Deny") => true,
            Some("Allow") => {
                // Fail closed, even for conditional wildcard grants. IAM identity policies grant worker access.
                statement.get("NotPrincipal").is_none()
                    && statement["Principal"]
                        .as_object()
                        .is_some_and(|principals| {
                            !principals.is_empty() && principals.values().all(explicit_principal)
                        })
            }
            _ => false,
        })
}

fn explicit_principal(value: &Value) -> bool {
    match value {
        Value::String(principal) => !principal.is_empty() && !principal.contains(['*', '?']),
        Value::Array(principals) => {
            !principals.is_empty()
                && principals
                    .iter()
                    .all(|principal| principal.is_string() && explicit_principal(principal))
        }
        _ => false,
    }
}

fn tls_denied(policy: &Value, arn: &str) -> bool {
    statements(policy).iter().any(|s| {
        s["Effect"] == "Deny"
            && (s["Principal"] == "*" || s["Principal"]["AWS"] == "*")
            && (contains(&s["Action"], "sqs:*") || contains(&s["Action"], "*"))
            && (contains(&s["Resource"], arn) || contains(&s["Resource"], "*"))
            && s.get("NotAction").is_none()
            && s.get("NotResource").is_none()
            && s.get("NotPrincipal").is_none()
            && s["Condition"].as_object().is_some_and(|c| c.len() == 1)
            && s["Condition"]["Bool"]
                .as_object()
                .is_some_and(|c| c.len() == 1)
            && (s["Condition"]["Bool"]["aws:SecureTransport"] == false
                || s["Condition"]["Bool"]["aws:SecureTransport"] == "false")
    })
}

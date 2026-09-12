use application::error::{BoxError, box_error};
mod llm_logging;
use futures::{StreamExt, stream};
use google_cloud_auth::credentials::AccessTokenCredentials;
use image_fetcher::{FetchedImage, ImageFetcher};
pub use llm_logging::{
    GeminiServiceTier, LlmInvocationMetrics, LlmModel, LlmOperation, LlmProvider,
    log_llm_invocation, log_llm_invocation_failure,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroUsize,
    sync::Arc,
    time::Duration,
};
use url::Url;

const MAX_IMAGES_PER_REQUEST: usize = 5;
const VERTEX_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_CONCURRENT_REQUESTS: NonZeroUsize = match NonZeroUsize::new(4) {
    Some(value) => value,
    None => NonZeroUsize::MIN,
};
const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexAiConfig {
    project_id: String,
    location: String,
    model: String,
}

impl VertexAiConfig {
    pub fn new(
        project_id: impl Into<String>,
        location: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            location: location.into(),
            model: model.into(),
        }
    }

    fn project_id(&self) -> &str {
        &self.project_id
    }

    fn location(&self) -> &str {
        &self.location
    }

    fn model(&self) -> &str {
        &self.model
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenerationOptions {
    pub temperature: f32,
    pub max_output_tokens: u16,
    pub request_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchGenerationOptions {
    pub max_concurrent_requests: NonZeroUsize,
}

impl BatchGenerationOptions {
    pub const fn new(max_concurrent_requests: NonZeroUsize) -> Self {
        Self {
            max_concurrent_requests,
        }
    }
}

impl Default for BatchGenerationOptions {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CONCURRENT_REQUESTS)
    }
}

#[derive(Debug, Clone)]
pub struct StructuredGenerationRequest {
    pub operation: LlmOperation,
    pub system_instruction: String,
    pub prompt: String,
    pub image_urls: Vec<Url>,
    pub response_json_schema: serde_json::Value,
    pub options: GenerationOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LargeLanguageModelRetryKind {
    RateLimited,
    ServiceUnavailable,
    Transient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargeLanguageModelRetryAdvice {
    pub kind: LargeLanguageModelRetryKind,
    pub retry_after: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuredResponseFailureKind {
    InvalidJson,
    TargetDeserialization,
    MissingRequiredField,
    MaxTokens,
    MissingContent,
}

impl StructuredResponseFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidJson => "invalid_json",
            Self::TargetDeserialization => "target_deserialization",
            Self::MissingRequiredField => "missing_required_field",
            Self::MaxTokens => "max_tokens",
            Self::MissingContent => "missing_content",
        }
    }
}

impl std::fmt::Display for StructuredResponseFailureKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LargeLanguageModelError {
    #[error("large language model request configuration is invalid")]
    InvalidRequest {
        #[source]
        source: BoxError,
    },
    #[error("large language model authentication failed")]
    Authentication {
        #[source]
        source: BoxError,
    },
    #[error("large language model request timed out")]
    Timeout {
        #[source]
        source: BoxError,
    },
    #[error("large language model is temporarily unavailable")]
    Retryable {
        advice: LargeLanguageModelRetryAdvice,
        #[source]
        source: BoxError,
    },
    #[error("large language model rejected the request")]
    Permanent {
        #[source]
        source: BoxError,
    },
    #[error("large language model returned an invalid response")]
    InvalidResponse {
        #[source]
        source: BoxError,
    },
}

impl LargeLanguageModelError {
    /// Return the safe structured-response failure category, when this error
    /// came from provider response parsing.
    pub fn structured_response_failure_kind(&self) -> Option<StructuredResponseFailureKind> {
        match self {
            Self::InvalidResponse { source } => source
                .downcast_ref::<StructuredResponseError>()
                .map(StructuredResponseError::failure_kind),
            _ => None,
        }
    }

    /// Return the bounded feedback code for a provider response that could not be
    /// converted into the requested structured output.
    pub fn response_feedback_code(&self) -> Option<&'static str> {
        match self {
            Self::InvalidResponse { source } => source
                .downcast_ref::<StructuredResponseError>()
                .map(StructuredResponseError::feedback_code),
            _ => None,
        }
    }

    /// Return bounded, schema-only correction context for a retry prompt.
    pub fn response_correction_feedback(&self) -> Option<String> {
        match self {
            Self::InvalidResponse { source } => source
                .downcast_ref::<StructuredResponseError>()
                .map(StructuredResponseError::correction_feedback),
            _ => None,
        }
    }

    pub fn retry_advice(&self) -> Option<LargeLanguageModelRetryAdvice> {
        match self {
            Self::Retryable { advice, .. } => Some(*advice),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "invalid_request",
            Self::Authentication { .. } => "authentication",
            Self::Timeout { .. } => "timeout",
            Self::Retryable { advice, .. } => match advice.kind {
                LargeLanguageModelRetryKind::RateLimited => "rate_limited",
                LargeLanguageModelRetryKind::ServiceUnavailable => "service_unavailable",
                LargeLanguageModelRetryKind::Transient => "transient",
            },
            Self::Permanent { .. } => "permanent",
            Self::InvalidResponse { .. } => "invalid_response",
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum StructuredResponseError {
    #[error("response JSON syntax is invalid")]
    JsonSyntax {
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "response does not deserialize into the requested schema at {json_path} ({error_kind})"
    )]
    TargetDeserialization {
        json_path: String,
        error_kind: &'static str,
        missing_field: Option<String>,
        #[source]
        source: serde_path_to_error::Error<serde_json::Error>,
    },
    #[error("response was truncated because generation reached MAX_TOKENS")]
    TruncatedMaxTokens,
    #[error("response contains no usable content")]
    MissingContent,
}

impl StructuredResponseError {
    const fn failure_kind(&self) -> StructuredResponseFailureKind {
        match self {
            Self::JsonSyntax { .. } => StructuredResponseFailureKind::InvalidJson,
            Self::TargetDeserialization { missing_field, .. } if missing_field.is_some() => {
                StructuredResponseFailureKind::MissingRequiredField
            }
            Self::TargetDeserialization { .. } => {
                StructuredResponseFailureKind::TargetDeserialization
            }
            Self::TruncatedMaxTokens => StructuredResponseFailureKind::MaxTokens,
            Self::MissingContent => StructuredResponseFailureKind::MissingContent,
        }
    }

    const fn feedback_code(&self) -> &'static str {
        match self {
            Self::JsonSyntax { .. } => "response_invalid_json",
            Self::TargetDeserialization { missing_field, .. } if missing_field.is_some() => {
                "response_missing_required_field"
            }
            Self::TargetDeserialization { .. } => "response_schema_deserialization_failed",
            Self::TruncatedMaxTokens => "response_truncated_max_tokens",
            Self::MissingContent => "response_missing_content",
        }
    }

    fn correction_feedback(&self) -> String {
        match self {
            Self::TargetDeserialization {
                json_path,
                missing_field: Some(missing_field),
                ..
            } => format!(
                "{} at {json_path}; missing_field={missing_field}",
                self.feedback_code()
            ),
            Self::TargetDeserialization { json_path, .. } => {
                format!("{} at {json_path}", self.feedback_code())
            }
            _ => self.feedback_code().to_owned(),
        }
    }
}

#[async_trait::async_trait]
pub trait LargeLanguageModel: Send + Sync {
    async fn generate<Output>(
        &self,
        request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send;

    async fn generate_batch<Output>(
        &self,
        requests: Vec<StructuredGenerationRequest>,
        options: BatchGenerationOptions,
    ) -> Vec<Result<Output, LargeLanguageModelError>>
    where
        Output: DeserializeOwned + Send,
    {
        stream::iter(requests)
            .map(|request| self.generate(request))
            .buffered(options.max_concurrent_requests.get())
            .collect()
            .await
    }
}

pub struct VertexAiGemini {
    config: VertexAiConfig,
    client: reqwest::Client,
    image_fetcher: ImageFetcher,
    credentials: AccessTokenCredentials,
}

impl VertexAiGemini {
    pub fn new(
        config: VertexAiConfig,
        credentials: AccessTokenCredentials,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(VERTEX_CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            config,
            client,
            image_fetcher: ImageFetcher::new(),
            credentials,
        })
    }
}

#[async_trait::async_trait]
impl LargeLanguageModel for VertexAiGemini {
    async fn generate<Output>(
        &self,
        request: StructuredGenerationRequest,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        let images = self.fetch_images(&request.image_urls).await;
        self.generate_with_images(request, images).await
    }

    async fn generate_batch<Output>(
        &self,
        requests: Vec<StructuredGenerationRequest>,
        options: BatchGenerationOptions,
    ) -> Vec<Result<Output, LargeLanguageModelError>>
    where
        Output: DeserializeOwned + Send,
    {
        let images = Arc::new(self.fetch_images_for_requests(&requests).await);
        stream::iter(requests)
            .map(|request| {
                let images = Arc::clone(&images);
                async move {
                    let request_images = images_for_request(&request, images.as_ref());
                    self.generate_with_images(request, request_images).await
                }
            })
            .buffered(options.max_concurrent_requests.get())
            .collect()
            .await
    }
}

impl VertexAiGemini {
    async fn generate_with_images<Output>(
        &self,
        request: StructuredGenerationRequest,
        images: Vec<FetchedImage>,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned + Send,
    {
        let access_token = self.credentials.access_token().await.map_err(|source| {
            LargeLanguageModelError::Authentication {
                source: box_error(source),
            }
        })?;
        let request_timeout = request.options.request_timeout;
        let url = build_generate_content_url(&self.config);
        let provider_request = ProviderGenerateContentRequest::try_new(request, images)?;
        let operation = provider_request.operation;
        let started_at = std::time::Instant::now();
        let response = self
            .client
            .post(url)
            .timeout(request_timeout)
            .bearer_auth(access_token.token)
            .json(&provider_request.body)
            .send()
            .await
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(parse_retry_after);
            let body = read_bounded_response_body(response).await;
            return Err(http_status_error_with_body(status, retry_after, &body));
        }
        let response = response
            .json::<GenerateContentResponse>()
            .await
            .map_err(invalid_response_error)?;
        let usage_metrics = response.usage_metrics();
        match response.into_output(operation, self.config.model()) {
            Ok(output) => {
                log_llm_invocation(
                    operation,
                    LlmProvider::Google,
                    log_model(self.config.model()),
                    started_at.elapsed(),
                    usage_metrics,
                );
                Ok(output)
            }
            Err(error) => {
                if let Some(failure_kind) = error.structured_response_failure_kind() {
                    log_llm_invocation_failure(
                        operation,
                        LlmProvider::Google,
                        log_model(self.config.model()),
                        started_at.elapsed(),
                        usage_metrics,
                        failure_kind.as_str(),
                    );
                }
                Err(error)
            }
        }
    }
}

impl VertexAiGemini {
    async fn fetch_images(&self, urls: &[Url]) -> Vec<FetchedImage> {
        let mut images = Vec::with_capacity(MAX_IMAGES_PER_REQUEST);
        for url in urls.iter().take(MAX_IMAGES_PER_REQUEST) {
            if let Some(image) = self.image_fetcher.fetch(url).await {
                images.push(image);
            }
        }
        images
    }

    async fn fetch_images_for_requests(
        &self,
        requests: &[StructuredGenerationRequest],
    ) -> HashMap<String, FetchedImage> {
        let mut unique_urls = Vec::new();
        let mut seen = HashSet::new();
        for request in requests {
            for url in request.image_urls.iter().take(MAX_IMAGES_PER_REQUEST) {
                if seen.insert(url.as_str()) {
                    unique_urls.push(url);
                }
            }
        }
        let mut images = HashMap::with_capacity(unique_urls.len());
        for url in unique_urls {
            if let Some(image) = self.image_fetcher.fetch(url).await {
                images.insert(url.as_str().to_owned(), image);
            }
        }
        images
    }
}

fn images_for_request(
    request: &StructuredGenerationRequest,
    images: &HashMap<String, FetchedImage>,
) -> Vec<FetchedImage> {
    request
        .image_urls
        .iter()
        .take(MAX_IMAGES_PER_REQUEST)
        .filter_map(|url| images.get(url.as_str()).cloned())
        .collect()
}

fn build_generate_content_url(config: &VertexAiConfig) -> String {
    let endpoint = match config.location() {
        "us" | "eu" => format!(
            "https://aiplatform.{}.rep.googleapis.com",
            config.location()
        ),
        "global" => "https://aiplatform.googleapis.com".to_owned(),
        location => format!("https://{location}-aiplatform.googleapis.com"),
    };
    format!(
        "{endpoint}/v1/projects/{}/locations/{}/publishers/google/models/{}:generateContent",
        config.project_id(),
        config.location(),
        config.model(),
    )
}

fn log_model(model: &str) -> LlmModel {
    match model {
        "gemini-3.1-flash-lite" => LlmModel::Gemini31FlashLite,
        "gemini-3.1-pro-preview" => LlmModel::Gemini31ProPreview,
        _ => LlmModel::Configured,
    }
}

fn request_error(source: reqwest::Error) -> LargeLanguageModelError {
    if source.is_timeout() {
        LargeLanguageModelError::Timeout {
            source: box_error(source),
        }
    } else {
        LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after: None,
            },
            source: box_error(source),
        }
    }
}

fn invalid_response_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> LargeLanguageModelError {
    LargeLanguageModelError::InvalidResponse {
        source: box_error(source),
    }
}

async fn read_bounded_response_body(mut response: reqwest::Response) -> String {
    let mut body = Vec::with_capacity(MAX_PROVIDER_ERROR_BODY_BYTES.min(1024));
    while let Ok(Some(chunk)) = response.chunk().await {
        let remaining = MAX_PROVIDER_ERROR_BODY_BYTES.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if body.len() >= MAX_PROVIDER_ERROR_BODY_BYTES {
            break;
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

fn parse_retry_after(value: &reqwest::header::HeaderValue) -> Option<Duration> {
    let value = value.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    date.duration_since(std::time::SystemTime::now()).ok()
}

fn provider_error_detail(body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value.get("error").cloned())
        .and_then(|error| {
            let code = error.get("code").and_then(serde_json::Value::as_i64);
            let status = error.get("status").and_then(serde_json::Value::as_str);
            let message = error.get("message").and_then(serde_json::Value::as_str);
            let detail = message.or(status)?;
            Some(match code {
                Some(code) => format!("code={code} message={detail}"),
                None => detail.to_owned(),
            })
        })
        .unwrap_or_else(|| "provider error body unavailable".to_owned());
    detail.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum VertexResponseJsonSchemaError {
    #[error("unsupported schema keyword {keyword} at {pointer}")]
    UnsupportedKeyword { pointer: String, keyword: String },
    #[error("invalid schema keyword {keyword} at {pointer}")]
    InvalidKeyword { pointer: String, keyword: String },
    #[error("$ref cannot have non-$ siblings at {pointer}")]
    RefWithSiblings { pointer: String },
}

fn normalize_vertex_response_json_schema(
    schema: serde_json::Value,
) -> Result<serde_json::Value, VertexResponseJsonSchemaError> {
    normalize_schema_node(schema, "#", true, false)
}

fn normalize_schema_node(
    mut node: serde_json::Value,
    pointer: &str,
    is_root: bool,
    in_composition_branch: bool,
) -> Result<serde_json::Value, VertexResponseJsonSchemaError> {
    let Some(object) = node.as_object_mut() else {
        return Ok(node);
    };

    if is_root {
        object.remove("$schema");
    }

    if let Some(constant) = object.remove("const") {
        if object.contains_key("enum") {
            return Err(VertexResponseJsonSchemaError::InvalidKeyword {
                pointer: pointer.to_owned(),
                keyword: "const with enum".to_owned(),
            });
        }
        object.insert("enum".to_owned(), serde_json::Value::Array(vec![constant]));
    }

    for keyword in [
        "$comment",
        "default",
        "examples",
        "example",
        "deprecated",
        "readOnly",
        "writeOnly",
        "contentEncoding",
        "contentMediaType",
    ] {
        object.remove(keyword);
    }

    // Close standalone typed DTOs, but keep composition branches open. A
    // flattened tagged union puts shared fields (for example `selector`) on
    // the parent object and discriminator fields on `oneOf` branches. Closing
    // either branch would incorrectly reject the other fields.
    if !in_composition_branch
        && object.contains_key("properties")
        && !object.contains_key("additionalProperties")
        && !object.contains_key("oneOf")
        && !object.contains_key("anyOf")
    {
        object.insert(
            "additionalProperties".to_owned(),
            serde_json::Value::Bool(false),
        );
    }

    let has_ref = object.contains_key("$ref");
    if has_ref
        && object
            .keys()
            .any(|key| !key.starts_with('$') && !matches!(key.as_str(), "title" | "description"))
    {
        return Err(VertexResponseJsonSchemaError::RefWithSiblings {
            pointer: pointer.to_owned(),
        });
    }

    for keyword in [
        "allOf",
        "not",
        "if",
        "then",
        "else",
        "patternProperties",
        "dependentSchemas",
        "contains",
        "unevaluatedProperties",
        "unevaluatedItems",
        "propertyNames",
        "minProperties",
        "maxProperties",
        "multipleOf",
        "exclusiveMinimum",
        "exclusiveMaximum",
    ] {
        if object.contains_key(keyword) {
            return Err(VertexResponseJsonSchemaError::UnsupportedKeyword {
                pointer: pointer.to_owned(),
                keyword: keyword.to_owned(),
            });
        }
    }

    let known = [
        "$id",
        "$defs",
        "$ref",
        "$anchor",
        "type",
        "format",
        "title",
        "description",
        "enum",
        "items",
        "prefixItems",
        "minItems",
        "maxItems",
        "minimum",
        "maximum",
        "anyOf",
        "oneOf",
        "properties",
        "additionalProperties",
        "required",
        "propertyOrdering",
    ];
    for key in object.keys() {
        if !known.contains(&key.as_str()) {
            return Err(VertexResponseJsonSchemaError::UnsupportedKeyword {
                pointer: pointer.to_owned(),
                keyword: key.clone(),
            });
        }
    }

    let keys = object.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        let child_pointer = format!("{pointer}/{}", escape_json_pointer(&key));
        match key.as_str() {
            "$defs" | "properties" => {
                let Some(map) = object
                    .get_mut(&key)
                    .and_then(serde_json::Value::as_object_mut)
                else {
                    return Err(VertexResponseJsonSchemaError::InvalidKeyword {
                        pointer: child_pointer,
                        keyword: key,
                    });
                };
                let names = map.keys().cloned().collect::<Vec<_>>();
                for name in names {
                    let name_pointer = format!("{child_pointer}/{}", escape_json_pointer(&name));
                    let child = map.remove(&name).ok_or_else(|| {
                        VertexResponseJsonSchemaError::InvalidKeyword {
                            pointer: name_pointer.clone(),
                            keyword: key.clone(),
                        }
                    })?;
                    map.insert(
                        name,
                        normalize_schema_node(child, &name_pointer, false, false)?,
                    );
                }
            }
            "items" | "additionalProperties" => {
                let Some(child) = object.remove(&key) else {
                    continue;
                };
                if child.is_object() {
                    object.insert(
                        key,
                        normalize_schema_node(child, &child_pointer, false, in_composition_branch)?,
                    );
                } else {
                    object.insert(key, child);
                }
            }
            "prefixItems" | "anyOf" | "oneOf" => {
                let Some(values) = object
                    .get_mut(&key)
                    .and_then(serde_json::Value::as_array_mut)
                else {
                    return Err(VertexResponseJsonSchemaError::InvalidKeyword {
                        pointer: child_pointer,
                        keyword: key,
                    });
                };
                for (index, child) in values.iter_mut().enumerate() {
                    let child_pointer = format!("{child_pointer}/{index}");
                    let normalized =
                        normalize_schema_node(child.take(), &child_pointer, false, true)?;
                    *child = normalized;
                }
            }
            _ => {}
        }
    }

    hoist_one_of_object_properties(object, pointer);
    Ok(node)
}

/// Vertex's structured-output generator can follow the parent object of a
/// flattened tagged union more reliably than `required` fields nested only in
/// `oneOf` branches. Hoist branch properties and the required-field
/// intersection to the parent while retaining the original branches for
/// conditional validation (for example, `attribute` requiring `name`).
fn hoist_one_of_object_properties(
    object: &mut serde_json::Map<String, serde_json::Value>,
    pointer: &str,
) {
    let Some(branches) = object
        .get("oneOf")
        .and_then(serde_json::Value::as_array)
        .cloned()
    else {
        return;
    };
    if object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .is_none()
    {
        return;
    }

    let branch_objects = branches
        .iter()
        .filter_map(serde_json::Value::as_object)
        .collect::<Vec<_>>();
    if branch_objects.len() != branches.len() || branch_objects.is_empty() {
        return;
    }
    let branch_properties = branch_objects
        .iter()
        .map(|branch| {
            branch
                .get("properties")
                .and_then(serde_json::Value::as_object)
        })
        .collect::<Option<Vec<_>>>();
    let Some(branch_properties) = branch_properties else {
        return;
    };
    let branch_required = branch_objects
        .iter()
        .map(|branch| {
            branch
                .get("required")
                .and_then(serde_json::Value::as_array)
                .map(|required| {
                    required
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .collect::<Vec<_>>()
                })
        })
        .collect::<Option<Vec<_>>>();
    let Some(branch_required) = branch_required else {
        return;
    };

    let mut names = Vec::new();
    for properties in &branch_properties {
        for name in properties.keys() {
            if !names.iter().any(|known| known == name) {
                names.push(name.clone());
            }
        }
    }

    let mut hoisted_properties = Vec::new();
    for name in names {
        let already_present = object
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|properties| properties.contains_key(&name));
        if already_present {
            continue;
        }
        let schemas = branch_properties
            .iter()
            .filter_map(|properties| properties.get(&name))
            .collect::<Vec<_>>();
        if let Some(schema) = merge_hoisted_property_schemas(&schemas) {
            hoisted_properties.push((name, schema));
        }
    }

    if let Some(parent_properties) = object
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    {
        for (name, schema) in hoisted_properties {
            parent_properties.insert(name, schema);
        }
    }

    let Some(parent_required) = object
        .get_mut("required")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    for name in &branch_required[0] {
        if branch_required
            .iter()
            .all(|required| required.iter().any(|field| field == name))
            && !parent_required
                .iter()
                .any(|field| field.as_str() == Some(name))
        {
            parent_required.push(serde_json::Value::String((*name).to_owned()));
        }
    }

    let has_discriminator = object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|properties| properties.contains_key("type"));
    let discriminator_is_required = object
        .get("required")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|required| required.iter().any(|field| field.as_str() == Some("type")));
    if has_discriminator && discriminator_is_required {
        object.insert(
            "additionalProperties".to_owned(),
            serde_json::Value::Bool(false),
        );
    } else {
        tracing::debug!(
            schema_pointer = pointer,
            "Could not hoist tagged-union discriminator"
        );
    }
}

fn merge_hoisted_property_schemas(schemas: &[&serde_json::Value]) -> Option<serde_json::Value> {
    let first = schemas.first()?.as_object()?;
    if schemas
        .iter()
        .all(|schema| schema.as_object() == Some(first))
    {
        return Some(serde_json::Value::Object(first.clone()));
    }

    let enums = schemas
        .iter()
        .map(|schema| schema.as_object()?.get("enum")?.as_array())
        .collect::<Option<Vec<_>>>()?;
    let mut merged = first.clone();
    merged.remove("enum");
    for schema in schemas {
        let mut comparable = schema.as_object()?.clone();
        comparable.remove("enum");
        if comparable != merged {
            return None;
        }
    }

    let mut values = Vec::new();
    for values_for_schema in enums {
        for value in values_for_schema {
            if !values.iter().any(|known| known == value) {
                values.push(value.clone());
            }
        }
    }
    merged.insert("enum".to_owned(), serde_json::Value::Array(values));
    Some(serde_json::Value::Object(merged))
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
fn http_status_error(status: reqwest::StatusCode) -> LargeLanguageModelError {
    http_status_error_with_body(status, None, "")
}

fn http_status_error_with_body(
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
    body: &str,
) -> LargeLanguageModelError {
    let detail = provider_error_detail(body);
    let source = std::io::Error::other(format!("Vertex AI returned HTTP {status}: {detail}"));
    match status.as_u16() {
        401 | 403 => LargeLanguageModelError::Authentication {
            source: box_error(source),
        },
        429 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::RateLimited,
                retry_after,
            },
            source: box_error(source),
        },
        503 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::ServiceUnavailable,
                retry_after,
            },
            source: box_error(source),
        },
        408 | 500 | 502 | 504 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after,
            },
            source: box_error(source),
        },
        status if status >= 500 => LargeLanguageModelError::Retryable {
            advice: LargeLanguageModelRetryAdvice {
                kind: LargeLanguageModelRetryKind::Transient,
                retry_after,
            },
            source: box_error(source),
        },
        _ => LargeLanguageModelError::Permanent {
            source: box_error(source),
        },
    }
}

struct ProviderGenerateContentRequest {
    operation: LlmOperation,
    body: GenerateContentBody,
}

impl ProviderGenerateContentRequest {
    fn try_new(
        request: StructuredGenerationRequest,
        images: Vec<FetchedImage>,
    ) -> Result<Self, LargeLanguageModelError> {
        let response_json_schema = normalize_vertex_response_json_schema(
            request.response_json_schema,
        )
        .map_err(|source| LargeLanguageModelError::InvalidRequest {
            source: box_error(source),
        })?;
        let mut contents = Vec::with_capacity(images.len() + 1);
        contents.push(ProviderContent::text(request.prompt));
        contents.extend(images.into_iter().map(ProviderContent::image));
        Ok(Self {
            operation: request.operation,
            body: GenerateContentBody {
                system_instruction: ProviderContent::system_text(request.system_instruction),
                contents,
                generation_config: GenerationConfig {
                    temperature: request.options.temperature,
                    max_output_tokens: request.options.max_output_tokens,
                    response_mime_type: "application/json",
                    response_json_schema,
                },
            },
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentBody {
    system_instruction: ProviderContent,
    contents: Vec<ProviderContent>,
    generation_config: GenerationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<ProviderPart>,
}

impl ProviderContent {
    fn text(text: impl Into<String>) -> Self {
        Self {
            role: Some("user".to_owned()),
            parts: vec![ProviderPart::Text {
                text: text.into(),
                thought: None,
            }],
        }
    }

    fn system_text(text: impl Into<String>) -> Self {
        Self {
            role: None,
            parts: vec![ProviderPart::Text {
                text: text.into(),
                thought: None,
            }],
        }
    }

    fn image(image: FetchedImage) -> Self {
        Self {
            role: Some("user".to_owned()),
            parts: vec![ProviderPart::InlineData {
                inline_data: ProviderInlineData {
                    mime_type: image.mime_type().to_owned(),
                    data: image.base64_data().to_owned(),
                },
            }],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum ProviderPart {
    Text {
        text: String,
        #[serde(default, skip_serializing)]
        thought: Option<bool>,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: ProviderInlineData,
    },
}

impl ProviderPart {
    fn into_text(self) -> Option<String> {
        match self {
            Self::Text { text, thought } if thought != Some(true) => Some(text),
            Self::InlineData { .. } => None,
            Self::Text { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderInlineData {
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    temperature: f32,
    max_output_tokens: u16,
    response_mime_type: &'static str,
    response_json_schema: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<ProviderCandidate>,
    #[serde(default)]
    usage_metadata: ProviderUsageMetadata,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
    cached_content_token_count: Option<u32>,
    thoughts_token_count: Option<u32>,
}

impl GenerateContentResponse {
    fn usage_metrics(&self) -> LlmInvocationMetrics {
        LlmInvocationMetrics {
            service_tier: Some(GeminiServiceTier::Standard),
            prompt_tokens: self.usage_metadata.prompt_token_count,
            completion_tokens: self.usage_metadata.candidates_token_count,
            total_tokens: self.usage_metadata.total_token_count,
            cached_prompt_tokens: self.usage_metadata.cached_content_token_count,
            reasoning_tokens: self.usage_metadata.thoughts_token_count,
            ..Default::default()
        }
    }

    fn into_output<Output>(
        self,
        operation: LlmOperation,
        model: &str,
    ) -> Result<Output, LargeLanguageModelError>
    where
        Output: DeserializeOwned,
    {
        let candidate_count = self.candidates.len();
        let candidate = self.candidates.into_iter().next();
        let (finish_reason, content) = candidate
            .map(|candidate| (candidate.finish_reason, candidate.content))
            .unwrap_or((None, None));
        let finish_reason = finish_reason.as_deref();
        let part_count = content.as_ref().map_or(0, |content| content.parts.len());
        let text = content
            .map(|content| {
                content
                    .parts
                    .into_iter()
                    .filter_map(ProviderPart::into_text)
                    .collect::<String>()
            })
            .unwrap_or_default();
        let text_length = text.len();

        if finish_reason == Some("MAX_TOKENS") {
            log_response_parse_failure(ResponseParseFailure {
                operation,
                model,
                finish_reason,
                candidate_count,
                part_count,
                text_length,
                parse_stage: "truncated_max_tokens",
                json_path: None,
                error_kind: "max_tokens",
                missing_field: None,
                json_error: None,
            });
            return Err(invalid_response_error(
                StructuredResponseError::TruncatedMaxTokens,
            ));
        }

        if text.trim().is_empty() {
            log_response_parse_failure(ResponseParseFailure {
                operation,
                model,
                finish_reason,
                candidate_count,
                part_count,
                text_length,
                parse_stage: "missing_content",
                json_path: None,
                error_kind: "missing_content",
                missing_field: None,
                json_error: None,
            });
            return Err(invalid_response_error(
                StructuredResponseError::MissingContent,
            ));
        }

        let text = strip_json_fence(&text);
        let value = match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => value,
            Err(error) => {
                log_response_parse_failure(ResponseParseFailure {
                    operation,
                    model,
                    finish_reason,
                    candidate_count,
                    part_count,
                    text_length,
                    parse_stage: "json_syntax",
                    json_path: None,
                    error_kind: serde_json_error_kind(&error),
                    missing_field: None,
                    json_error: Some(&error),
                });
                return Err(invalid_response_error(
                    StructuredResponseError::JsonSyntax { source: error },
                ));
            }
        };

        match serde_path_to_error::deserialize::<_, Output>(value) {
            Ok(output) => Ok(output),
            Err(error) => {
                let json_path = error.path().to_string();
                let missing_field = missing_field_name(error.inner());
                let error_kind = if missing_field.is_some() {
                    "missing_field"
                } else {
                    target_deserialization_error_kind(error.inner())
                };
                log_response_parse_failure(ResponseParseFailure {
                    operation,
                    model,
                    finish_reason,
                    candidate_count,
                    part_count,
                    text_length,
                    parse_stage: "target_deserialization",
                    json_path: Some(&json_path),
                    error_kind,
                    missing_field: missing_field.as_deref(),
                    json_error: None,
                });
                Err(invalid_response_error(
                    StructuredResponseError::TargetDeserialization {
                        json_path,
                        error_kind,
                        missing_field,
                        source: error,
                    },
                ))
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCandidate {
    content: Option<ProviderContent>,
    finish_reason: Option<String>,
}

fn strip_json_fence(text: &str) -> &str {
    let text = text.trim();
    let Some(text) = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
    else {
        return text;
    };
    text.strip_suffix("```").map_or(text, str::trim)
}

struct ResponseParseFailure<'a> {
    operation: LlmOperation,
    model: &'a str,
    finish_reason: Option<&'a str>,
    candidate_count: usize,
    part_count: usize,
    text_length: usize,
    parse_stage: &'static str,
    json_path: Option<&'a str>,
    error_kind: &'static str,
    missing_field: Option<&'a str>,
    json_error: Option<&'a serde_json::Error>,
}

fn log_response_parse_failure(failure: ResponseParseFailure<'_>) {
    tracing::warn!(
        operation = %failure.operation,
        model = failure.model,
        finish_reason = failure.finish_reason.unwrap_or("UNSPECIFIED"),
        candidate_count = failure.candidate_count,
        part_count = failure.part_count,
        text_length = failure.text_length,
        parse_stage = failure.parse_stage,
        json_path = failure.json_path,
        error_kind = failure.error_kind,
        missing_field = failure.missing_field,
        json_line = failure.json_error.map(serde_json::Error::line),
        json_column = failure.json_error.map(serde_json::Error::column),
        "Vertex AI response could not be parsed as structured output"
    );
}

fn missing_field_name(error: &serde_json::Error) -> Option<String> {
    let message = error.to_string();
    let field = message.strip_prefix("missing field `")?;
    let end = field.find('`')?;
    Some(field[..end].to_owned())
}

fn serde_json_error_kind(error: &serde_json::Error) -> &'static str {
    match error.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    }
}

fn target_deserialization_error_kind(error: &serde_json::Error) -> &'static str {
    let message = error.to_string();
    if message.starts_with("invalid type") {
        "invalid_type"
    } else if message.starts_with("missing field") {
        "missing_field"
    } else if message.starts_with("unknown field") {
        "unknown_field"
    } else if message.starts_with("unknown variant") {
        "unknown_variant"
    } else {
        "deserialization"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_build_regional_and_global_vertex_endpoints_for_configured_model() {
        let config = VertexAiConfig::new("project", "eu", "gemini-3.1-flash-lite");
        assert_eq!(
            build_generate_content_url(&config),
            "https://aiplatform.eu.rep.googleapis.com/v1/projects/project/locations/eu/publishers/google/models/gemini-3.1-flash-lite:generateContent"
        );
        let config = VertexAiConfig::new("project", "global", "configured-model");
        assert_eq!(
            build_generate_content_url(&config),
            "https://aiplatform.googleapis.com/v1/projects/project/locations/global/publishers/google/models/configured-model:generateContent"
        );
        assert_eq!(
            log_model("gemini-3.1-pro-preview"),
            LlmModel::Gemini31ProPreview
        );
    }

    #[test]
    fn should_classify_transient_and_permanent_vertex_statuses() {
        assert!(matches!(
            http_status_error(reqwest::StatusCode::TOO_MANY_REQUESTS),
            LargeLanguageModelError::Retryable {
                advice: LargeLanguageModelRetryAdvice {
                    kind: LargeLanguageModelRetryKind::RateLimited,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            LargeLanguageModelError::Retryable {
                advice: LargeLanguageModelRetryAdvice {
                    kind: LargeLanguageModelRetryKind::ServiceUnavailable,
                    ..
                },
                ..
            }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::BAD_REQUEST),
            LargeLanguageModelError::Permanent { .. }
        ));
        assert!(matches!(
            http_status_error(reqwest::StatusCode::UNAUTHORIZED),
            LargeLanguageModelError::Authentication { .. }
        ));
    }

    #[test]
    fn should_preserve_caller_generation_options() {
        let request = StructuredGenerationRequest {
            operation: LlmOperation::ProductEnhancedSearchDescriptionMatching,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({"type": "object"}),
            options: GenerationOptions {
                temperature: 0.7,
                max_output_tokens: 512,
                request_timeout: Duration::from_secs(60),
            },
        };

        let request = ProviderGenerateContentRequest::try_new(request, Vec::new())
            .unwrap_or_else(|error| panic!("request should normalize: {error}"));

        assert_eq!(0.7, request.body.generation_config.temperature);
        assert_eq!(512, request.body.generation_config.max_output_tokens);
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct CallerDefinedOutput {
        matched: bool,
    }

    #[allow(dead_code)]
    #[derive(Debug, Deserialize)]
    struct NestedOutput {
        schemas: Vec<NestedSchema>,
    }

    #[allow(dead_code)]
    #[derive(Debug, Deserialize)]
    struct NestedSchema {
        price: NestedPrice,
    }

    #[allow(dead_code)]
    #[derive(Debug, Deserialize)]
    struct NestedPrice {
        selector: String,
    }

    struct OrderedTestLargeLanguageModel;

    #[async_trait::async_trait]
    impl LargeLanguageModel for OrderedTestLargeLanguageModel {
        async fn generate<Output>(
            &self,
            request: StructuredGenerationRequest,
        ) -> Result<Output, LargeLanguageModelError>
        where
            Output: DeserializeOwned + Send,
        {
            if request.prompt == "slow" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            serde_json::from_value(serde_json::json!({"matched": request.prompt == "fast"}))
                .map_err(invalid_response_error)
        }
    }

    fn test_request(prompt: &str) -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            operation: LlmOperation::ProductEnhancedSearchDescriptionMatching,
            system_instruction: "system".to_owned(),
            prompt: prompt.to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({"type": "object"}),
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 1,
                request_timeout: Duration::from_secs(60),
            },
        }
    }

    #[tokio::test]
    async fn should_preserve_batch_request_order_with_concurrency()
    -> Result<(), LargeLanguageModelError> {
        let results = OrderedTestLargeLanguageModel
            .generate_batch::<CallerDefinedOutput>(
                vec![test_request("slow"), test_request("fast")],
                BatchGenerationOptions::new(NonZeroUsize::new(2).unwrap_or(NonZeroUsize::MIN)),
            )
            .await;

        assert_eq!(
            vec![
                CallerDefinedOutput { matched: false },
                CallerDefinedOutput { matched: true },
            ],
            results.into_iter().collect::<Result<Vec<_>, _>>()?
        );
        Ok(())
    }

    #[test]
    fn should_serialize_standard_json_schema_without_vertex_openapi_schema() {
        let request = StructuredGenerationRequest {
            operation: LlmOperation::CrawlerUrlClassification,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "matched": {"const": true}
                }
            }),
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 32,
                request_timeout: Duration::from_secs(60),
            },
        };
        let provider = ProviderGenerateContentRequest::try_new(request, Vec::new())
            .unwrap_or_else(|error| panic!("request should normalize: {error}"));
        let body = serde_json::to_value(provider.body)
            .unwrap_or_else(|error| panic!("provider body should serialize: {error}"));
        assert_eq!(body["contents"][0]["role"], "user");
        assert!(body["systemInstruction"].get("role").is_none());
        let generation_config = &body["generationConfig"];
        assert_eq!(generation_config["responseMimeType"], "application/json");
        assert_eq!(generation_config["responseJsonSchema"]["type"], "object");
        assert_eq!(
            generation_config["responseJsonSchema"]["properties"]["matched"]["enum"],
            serde_json::json!([true])
        );
        assert!(generation_config.get("responseSchema").is_none());
        assert!(
            generation_config["responseJsonSchema"]
                .get("$schema")
                .is_none()
        );
    }

    #[test]
    fn should_reject_unsupported_schema_keywords_before_http() {
        let request = StructuredGenerationRequest {
            operation: LlmOperation::CrawlerUrlClassification,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_json_schema: serde_json::json!({"type": "object", "allOf": []}),
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 32,
                request_timeout: Duration::from_secs(60),
            },
        };
        assert!(matches!(
            ProviderGenerateContentRequest::try_new(request, Vec::new()),
            Err(LargeLanguageModelError::InvalidRequest { .. })
        ));
    }

    #[allow(dead_code)]
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    struct TestObject {
        matched: bool,
    }

    #[allow(dead_code)]
    #[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
    #[serde(tag = "mapping_type", rename_all = "snake_case")]
    enum TestTaggedResponse {
        State { state: String },
        Regex { pattern: String, state: String },
    }

    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct Child {
        value: String,
    }

    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct Parent {
        #[schemars(description = "Documented custom field")]
        child: Child,
    }

    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct NestedExtractionRule {
        selector: String,
        #[serde(flatten)]
        extraction: NestedExtractionKind,
        #[serde(default)]
        cardinality: NestedExtractionCardinality,
    }

    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum NestedExtractionKind {
        Text,
        Attribute { name: String },
        ImageUrl,
    }

    #[allow(dead_code)]
    #[derive(Default, schemars::JsonSchema)]
    enum NestedExtractionCardinality {
        #[default]
        First,
        All,
    }

    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct NestedProductSchema {
        #[schemars(description = "Title extraction rule")]
        title: NestedExtractionRule,
        #[schemars(description = "Image extraction rules")]
        images: Vec<NestedExtractionRule>,
    }

    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct NestedFlattenedResponse {
        schemas: Vec<NestedProductSchema>,
        #[serde(flatten)]
        evaluation: NestedEvaluation,
    }

    #[allow(dead_code)]
    #[derive(schemars::JsonSchema)]
    struct NestedEvaluation {
        confidence: String,
        summary: String,
    }

    fn request_with_response_json_schema(
        response_json_schema: serde_json::Value,
    ) -> StructuredGenerationRequest {
        StructuredGenerationRequest {
            operation: LlmOperation::CrawlerProductSchemaGeneration,
            system_instruction: "system".to_owned(),
            prompt: "prompt".to_owned(),
            image_urls: Vec::new(),
            response_json_schema,
            options: GenerationOptions {
                temperature: 0.0,
                max_output_tokens: 32,
                request_timeout: Duration::from_secs(60),
            },
        }
    }

    #[test]
    fn should_accept_documented_custom_type_references_in_provider_requests() {
        let response_json_schema = serde_json::to_value(schemars::schema_for!(Parent))
            .unwrap_or_else(|error| panic!("parent schema should serialize: {error}"));

        let provider = ProviderGenerateContentRequest::try_new(
            request_with_response_json_schema(response_json_schema),
            Vec::new(),
        )
        .unwrap_or_else(|error| panic!("provider request should normalize: {error}"));
        let body = serde_json::to_value(provider.body)
            .unwrap_or_else(|error| panic!("provider body should serialize: {error}"));
        let child = &body["generationConfig"]["responseJsonSchema"]["properties"]["child"];

        assert_eq!(child["$ref"], "#/$defs/Child");
        assert_eq!(child["description"], "Documented custom field");
    }

    #[test]
    fn should_accept_nested_flattened_response_schemas_in_provider_requests() {
        let response_json_schema =
            serde_json::to_value(schemars::schema_for!(NestedFlattenedResponse))
                .unwrap_or_else(|error| panic!("nested schema should serialize: {error}"));

        let provider = ProviderGenerateContentRequest::try_new(
            request_with_response_json_schema(response_json_schema),
            Vec::new(),
        )
        .unwrap_or_else(|error| panic!("nested schema should normalize: {error}"));
        let body = serde_json::to_value(provider.body)
            .unwrap_or_else(|error| panic!("provider body should serialize: {error}"));
        let rule = &body["generationConfig"]["responseJsonSchema"]["$defs"]["NestedExtractionRule"];
        assert_eq!(
            rule["properties"]["type"]["enum"],
            serde_json::json!(["text", "attribute", "image_url"])
        );
        assert!(
            rule["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|value| value == "type"))
        );
        assert_eq!(rule["additionalProperties"], serde_json::Value::Bool(false));
    }

    fn representative_product_schema_response_json_schema() -> serde_json::Value {
        serde_json::json!({
            "$defs": {
                "ExtractionRule": {
                    "type": "object",
                    "oneOf": [
                        {"type": "object", "properties": {"type": {"const": "text"}}, "required": ["type"]},
                        {"type": "object", "properties": {"type": {"const": "attribute"}, "name": {"type": "string"}}, "required": ["type", "name"]},
                        {"type": "object", "properties": {"type": {"const": "image_url"}}, "required": ["type"]}
                    ],
                    "properties": {
                        "selector": {"type": "string"},
                        "additional_selectors": {"type": "array", "items": {"type": "string"}},
                        "cardinality": {"$ref": "#/$defs/ExtractionCardinality"}
                    },
                    "required": ["selector"]
                },
                "ExtractionCardinality": {
                    "type": "string",
                    "enum": ["first", "all"]
                },
                "ProductCssSelectorSchema": {
                    "type": "object",
                    "properties": {
                        "title": {"$ref": "#/$defs/ExtractionRule"},
                        "state": {"$ref": "#/$defs/ExtractionRule"},
                        "images": {"$ref": "#/$defs/ExtractionRule"},
                        "description": {"anyOf": [{"$ref": "#/$defs/ExtractionRule"}, {"type": "null"}]},
                        "price": {"anyOf": [{"$ref": "#/$defs/ExtractionRule"}, {"type": "null"}]},
                        "source_listing_id": {"anyOf": [{"$ref": "#/$defs/ExtractionRule"}, {"type": "null"}]},
                        "lot_bidding_opens": {"anyOf": [{"$ref": "#/$defs/ExtractionRule"}, {"type": "null"}]},
                        "lot_scheduled_closes": {"anyOf": [{"$ref": "#/$defs/ExtractionRule"}, {"type": "null"}]},
                        "raw_attributes": {"type": "object", "additionalProperties": {"$ref": "#/$defs/ExtractionRule"}}
                    },
                    "required": ["title", "state", "images"]
                }
            },
            "type": "object",
            "properties": {
                "schemas": {"type": "array", "items": {"$ref": "#/$defs/ProductCssSelectorSchema"}},
                "confidence": {"type": "string"},
                "summary": {"type": "string"}
            },
            "required": ["schemas", "confidence", "summary"]
        })
    }

    #[test]
    fn should_preserve_product_schema_required_fields_when_normalizing_for_vertex() {
        let normalized = normalize_vertex_response_json_schema(
            representative_product_schema_response_json_schema(),
        )
        .unwrap_or_else(|error| panic!("schema should normalize: {error}"));

        let product = &normalized["$defs"]["ProductCssSelectorSchema"];
        let required = product["required"]
            .as_array()
            .unwrap_or_else(|| panic!("product schema should have required fields"));
        for field in ["title", "state", "images"] {
            assert!(required.iter().any(|value| value == field));
        }
        for optional in [
            "description",
            "price",
            "source_listing_id",
            "lot_bidding_opens",
            "lot_scheduled_closes",
        ] {
            assert!(!required.iter().any(|value| value == optional));
        }
        assert_eq!(
            product["additionalProperties"],
            serde_json::Value::Bool(false)
        );

        let rule = &normalized["$defs"]["ExtractionRule"];
        assert!(
            rule["required"]
                .as_array()
                .is_some_and(|required| { required.iter().any(|value| value == "selector") })
        );
        assert!(
            rule["required"]
                .as_array()
                .is_some_and(|required| { required.iter().any(|value| value == "type") })
        );
        assert_eq!(
            rule["properties"]["type"]["enum"],
            serde_json::json!(["text", "attribute", "image_url"])
        );
        assert!(rule["properties"].get("name").is_some());
        assert_eq!(rule["additionalProperties"], serde_json::Value::Bool(false));
        let variants = rule["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("flattened extraction variants should be preserved"));
        assert_eq!(variants.len(), 3);
        assert!(variants.iter().all(|variant| {
            variant["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|value| value == "type"))
                && variant.get("additionalProperties").is_none()
        }));
        assert!(variants.iter().any(|variant| {
            variant["required"].as_array().is_some_and(|required| {
                required.iter().any(|value| value == "type")
                    && required.iter().any(|value| value == "name")
            })
        }));
        let types = variants
            .iter()
            .filter_map(|variant| variant["properties"]["type"]["enum"][0].as_str())
            .collect::<Vec<_>>();
        assert_eq!(types, vec!["text", "attribute", "image_url"]);

        let raw_attributes = &product["properties"]["raw_attributes"];
        assert!(raw_attributes["additionalProperties"].is_object());
    }

    #[test]
    fn should_strip_unsupported_ref_annotations_and_reject_validation_siblings() {
        let annotations = serde_json::json!({
            "$ref": "#/$defs/Child",
            "$comment": "local documentation",
            "default": {"value": "fallback"},
            "examples": [{"value": "example"}],
            "deprecated": true,
            "readOnly": true,
            "writeOnly": true,
            "contentEncoding": "base64",
            "contentMediaType": "text/plain",
            "title": "Child value",
            "description": "Child documentation"
        });
        let normalized = normalize_vertex_response_json_schema(annotations)
            .unwrap_or_else(|error| panic!("annotations should normalize: {error}"));

        assert_eq!(normalized["title"], "Child value");
        assert_eq!(normalized["description"], "Child documentation");
        for keyword in [
            "$comment",
            "default",
            "examples",
            "deprecated",
            "readOnly",
            "writeOnly",
            "contentEncoding",
            "contentMediaType",
        ] {
            assert!(normalized.get(keyword).is_none());
        }
        assert!(matches!(
            normalize_vertex_response_json_schema(serde_json::json!({
                "$ref": "#/$defs/Child",
                "minLength": 1
            })),
            Err(VertexResponseJsonSchemaError::RefWithSiblings { .. })
        ));
    }

    #[test]
    fn should_normalize_representative_schemars_json_schemas() {
        let object = serde_json::to_value(schemars::schema_for!(TestObject))
            .unwrap_or_else(|error| panic!("object schema should serialize: {error}"));
        let tagged = serde_json::to_value(schemars::schema_for!(TestTaggedResponse))
            .unwrap_or_else(|error| panic!("tagged schema should serialize: {error}"));
        assert!(normalize_vertex_response_json_schema(object).is_ok());
        assert!(normalize_vertex_response_json_schema(tagged).is_ok());
    }

    #[test]
    fn should_parse_retry_after_delta_and_http_date() {
        let delta = reqwest::header::HeaderValue::from_static("7");
        assert_eq!(parse_retry_after(&delta), Some(Duration::from_secs(7)));
        let date = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(2));
        let date = reqwest::header::HeaderValue::from_str(&date)
            .unwrap_or_else(|error| panic!("date header should be valid: {error}"));
        assert!(parse_retry_after(&date).is_some_and(|delay| delay <= Duration::from_secs(2)));
    }

    fn response_with_parts(
        parts: Vec<ProviderPart>,
        finish_reason: Option<&str>,
    ) -> GenerateContentResponse {
        GenerateContentResponse {
            candidates: vec![ProviderCandidate {
                content: Some(ProviderContent {
                    role: Some("model".to_owned()),
                    parts,
                }),
                finish_reason: finish_reason.map(str::to_owned),
            }],
            usage_metadata: ProviderUsageMetadata::default(),
        }
    }

    fn json_part(text: &str) -> ProviderPart {
        ProviderPart::Text {
            text: text.to_owned(),
            thought: None,
        }
    }

    fn thought_part(text: &str) -> ProviderPart {
        ProviderPart::Text {
            text: text.to_owned(),
            thought: Some(true),
        }
    }

    #[test]
    fn should_deserialize_caller_defined_output_type() -> Result<(), LargeLanguageModelError> {
        let response = response_with_parts(vec![json_part(r#"{"matched":true}"#)], Some("STOP"));

        assert_eq!(
            response.into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model"
            )?,
            CallerDefinedOutput { matched: true }
        );
        Ok(())
    }

    #[test]
    fn should_deserialize_vertex_usage_metadata_with_all_token_fields() {
        let response: GenerateContentResponse = serde_json::from_value(serde_json::json!({
            "candidates": [],
            "usageMetadata": {
                "promptTokenCount": 11,
                "candidatesTokenCount": 7,
                "totalTokenCount": 18,
                "cachedContentTokenCount": 3,
                "thoughtsTokenCount": 5
            }
        }))
        .unwrap_or_else(|error| panic!("response should deserialize: {error}"));

        let metrics = response.usage_metrics();
        assert_eq!(metrics.prompt_tokens, Some(11));
        assert_eq!(metrics.completion_tokens, Some(7));
        assert_eq!(metrics.total_tokens, Some(18));
        assert_eq!(metrics.cached_prompt_tokens, Some(3));
        assert_eq!(metrics.reasoning_tokens, Some(5));
    }

    #[test]
    fn should_concatenate_multipart_json_response_text() -> Result<(), LargeLanguageModelError> {
        let response = response_with_parts(
            vec![
                json_part(r#"{"matched":"#),
                ProviderPart::InlineData {
                    inline_data: ProviderInlineData {
                        mime_type: "image/png".to_owned(),
                        data: "ignored".to_owned(),
                    },
                },
                json_part("true}"),
            ],
            Some("STOP"),
        );

        assert_eq!(
            response.into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model"
            )?,
            CallerDefinedOutput { matched: true }
        );
        Ok(())
    }

    #[test]
    fn should_ignore_thought_text_before_json() -> Result<(), LargeLanguageModelError> {
        let response: GenerateContentResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "finishReason": "STOP",
                "content": {
                    "role": "model",
                    "parts": [
                        {"text": "internal reasoning", "thought": true},
                        {"text": "{\"matched\":true}"}
                    ]
                }
            }]
        }))
        .unwrap_or_else(|error| panic!("response should deserialize: {error}"));

        assert_eq!(
            response.into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model"
            )?,
            CallerDefinedOutput { matched: true }
        );
        Ok(())
    }

    #[test]
    fn should_ignore_thought_text_and_concatenate_json_parts() -> Result<(), LargeLanguageModelError>
    {
        let response = response_with_parts(
            vec![
                thought_part("internal reasoning"),
                json_part(r#"{"matched":"#),
                json_part("true}"),
            ],
            Some("STOP"),
        );

        assert_eq!(
            response.into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model"
            )?,
            CallerDefinedOutput { matched: true }
        );
        Ok(())
    }

    #[test]
    fn should_parse_json_markdown_fences() -> Result<(), LargeLanguageModelError> {
        for fenced in [
            "```json\n{\"matched\":true}\n```",
            "```\n{\"matched\":true}\n```",
        ] {
            let response = response_with_parts(vec![json_part(fenced)], Some("STOP"));
            assert_eq!(
                response.into_output::<CallerDefinedOutput>(
                    LlmOperation::CrawlerProductSchemaGeneration,
                    "test-model"
                )?,
                CallerDefinedOutput { matched: true }
            );
        }
        Ok(())
    }

    #[test]
    fn should_deserialize_stop_finish_reason_and_parse_output()
    -> Result<(), LargeLanguageModelError> {
        let response: GenerateContentResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "finishReason": "STOP",
                "content": {
                    "role": "model",
                    "parts": [{"text": "{\"matched\":true}"}]
                }
            }]
        }))
        .unwrap_or_else(|error| panic!("response should deserialize: {error}"));

        assert_eq!(
            response.into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model"
            )?,
            CallerDefinedOutput { matched: true }
        );
        Ok(())
    }

    #[test]
    fn should_reject_max_tokens_before_parsing_truncated_json() {
        let response: GenerateContentResponse = serde_json::from_value(serde_json::json!({
            "candidates": [{
                "finishReason": "MAX_TOKENS",
                "content": {
                    "role": "model",
                    "parts": [{"text": "{\"matched\":"}]
                }
            }],
            "usageMetadata": {
                "promptTokenCount": 101,
                "candidatesTokenCount": 8192,
                "totalTokenCount": 8293,
                "thoughtsTokenCount": 64
            }
        }))
        .unwrap_or_else(|error| panic!("response should deserialize: {error}"));

        let metrics = response.usage_metrics();
        assert_eq!(metrics.prompt_tokens, Some(101));
        assert_eq!(metrics.completion_tokens, Some(8192));
        assert_eq!(metrics.total_tokens, Some(8293));
        assert_eq!(metrics.reasoning_tokens, Some(64));

        let error = response
            .into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model",
            )
            .expect_err("MAX_TOKENS should reject the response");
        assert_eq!(
            error.response_feedback_code(),
            Some("response_truncated_max_tokens")
        );
        assert_eq!(
            error.structured_response_failure_kind(),
            Some(StructuredResponseFailureKind::MaxTokens)
        );
        assert!(matches!(
            error,
            LargeLanguageModelError::InvalidResponse { .. }
        ));
    }

    #[test]
    fn should_classify_missing_content_separately() {
        let response = response_with_parts(vec![thought_part("internal reasoning")], Some("STOP"));
        let error = response
            .into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model",
            )
            .expect_err("thought-only response should have no usable content");

        assert_eq!(
            error.response_feedback_code(),
            Some("response_missing_content")
        );
    }

    #[test]
    fn should_distinguish_json_syntax_from_target_deserialization_failures() {
        let invalid_json = response_with_parts(vec![json_part("{\"matched\":")], Some("STOP"));
        let syntax_error = invalid_json
            .into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model",
            )
            .expect_err("invalid JSON should fail at syntax parsing");
        assert_eq!(
            syntax_error.response_feedback_code(),
            Some("response_invalid_json")
        );
        assert!(format!("{syntax_error:?}").contains("JsonSyntax"));

        let wrong_type = response_with_parts(vec![json_part(r#"{"matched":"yes"}"#)], Some("STOP"));
        let target_error = wrong_type
            .into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model",
            )
            .expect_err("wrong field type should fail target deserialization");
        assert_eq!(
            target_error.response_feedback_code(),
            Some("response_schema_deserialization_failed")
        );
        let target_debug = format!("{target_error:?}");
        assert!(target_debug.contains("TargetDeserialization"));
        assert!(target_debug.contains("matched"));
    }

    #[test]
    fn should_report_missing_required_field_as_target_deserialization() {
        let response = response_with_parts(vec![json_part(r#"{"other":true}"#)], Some("STOP"));
        let error = response
            .into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model",
            )
            .expect_err("missing required field should fail target deserialization");

        assert_eq!(
            error.response_feedback_code(),
            Some("response_missing_required_field")
        );
        assert!(format!("{error:?}").contains("missing_field"));
        assert_eq!(
            error.response_correction_feedback().as_deref(),
            Some("response_missing_required_field at .; missing_field=matched")
        );
    }

    #[test]
    fn should_report_nested_target_deserialization_path() {
        let response = response_with_parts(
            vec![json_part(r#"{"schemas":[{"price":{"selector":7}}]}"#)],
            Some("STOP"),
        );
        let error = response
            .into_output::<NestedOutput>(LlmOperation::CrawlerProductSchemaGeneration, "test-model")
            .expect_err("nested wrong field type should fail target deserialization");

        let debug = format!("{error:?}");
        assert_eq!(
            error.response_feedback_code(),
            Some("response_schema_deserialization_failed")
        );
        assert!(debug.contains("schemas[0].price.selector"));
        assert!(debug.contains("invalid_type"));
    }

    #[test]
    fn should_report_nested_missing_field_name_without_model_content() {
        let response = response_with_parts(
            vec![json_part(r#"{"schemas":[{"price":{}}]}"#)],
            Some("STOP"),
        );
        let error = response
            .into_output::<NestedOutput>(LlmOperation::CrawlerProductSchemaGeneration, "test-model")
            .expect_err("nested missing field should fail target deserialization");

        assert_eq!(
            error.response_feedback_code(),
            Some("response_missing_required_field")
        );
        assert_eq!(
            error.response_correction_feedback().as_deref(),
            Some("response_missing_required_field at schemas[0].price; missing_field=selector")
        );
    }

    #[test]
    fn should_keep_malformed_response_diagnostics_free_of_model_content() {
        let marker = "do-not-log-this-model-content";
        let response = response_with_parts(
            vec![json_part(&format!("{{\"matched\":{marker}"))],
            Some("STOP"),
        );

        let error = response
            .into_output::<CallerDefinedOutput>(
                LlmOperation::CrawlerProductSchemaGeneration,
                "test-model",
            )
            .expect_err("malformed JSON should fail");
        assert!(!format!("{error:?}").contains(marker));
    }
}

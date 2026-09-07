use google_cloud_auth::credentials::{AccessTokenCredentials, Builder as GoogleCredentialsBuilder};
use large_language_model::{VertexAiConfig, VertexAiGemini};

const GOOGLE_CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const DEFAULT_MODEL: &str = "gemini-3.1-pro-preview";
const DEFAULT_CHEAP_MODEL: &str = "gemini-3.1-flash-lite";

/// Provider-specific model selection used only by crawler executable wiring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlerVertexAiModels {
    pub product_schema: String,
    pub url_classification: String,
}

impl CrawlerVertexAiModels {
    pub fn from_env() -> Self {
        let product_schema = env_or_default("VERTEX_AI_MODEL", DEFAULT_MODEL);
        let cheap_model = env_or_default("CRAWLER_VERTEX_AI_CHEAP_MODEL", DEFAULT_CHEAP_MODEL);

        Self {
            product_schema,
            url_classification: env_or_default(
                "CRAWLER_VERTEX_AI_URL_CLASSIFICATION_MODEL",
                &cheap_model,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlerVertexAiConfig {
    project_id: String,
    location: String,
}

#[derive(Debug, thiserror::Error)]
pub enum CrawlerVertexAiConfigError {
    #[error("missing required environment variable: {name}")]
    MissingEnvironment { name: &'static str },
    #[error("failed to initialize Google application default credentials")]
    Credentials,
    #[error("failed to initialize Vertex AI client")]
    Client,
}

impl CrawlerVertexAiConfig {
    pub fn from_env() -> Result<Self, CrawlerVertexAiConfigError> {
        Ok(Self {
            project_id: required_env("VERTEX_AI_PROJECT_ID")?,
            location: required_env("VERTEX_AI_LOCATION")?,
        })
    }

    pub fn create_model(
        &self,
        model: impl Into<String>,
    ) -> Result<VertexAiGemini, CrawlerVertexAiConfigError> {
        let credentials = application_default_credentials()?;
        VertexAiGemini::new(
            VertexAiConfig::new(self.project_id.clone(), self.location.clone(), model),
            credentials,
        )
        .map_err(|_| CrawlerVertexAiConfigError::Client)
    }
}

fn required_env(name: &'static str) -> Result<String, CrawlerVertexAiConfigError> {
    std::env::var(name).map_err(|_| CrawlerVertexAiConfigError::MissingEnvironment { name })
}

fn env_or_default(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn application_default_credentials() -> Result<AccessTokenCredentials, CrawlerVertexAiConfigError> {
    GoogleCredentialsBuilder::default()
        .with_scopes([GOOGLE_CLOUD_PLATFORM_SCOPE])
        .build_access_token_credentials()
        .map_err(|_| CrawlerVertexAiConfigError::Credentials)
}

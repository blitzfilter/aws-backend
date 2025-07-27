use crate::IntegrationTestService;
use crate::localstack::get_aws_config;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_opensearch::operation::create_domain::{CreateDomainError, CreateDomainOutput};
use aws_sdk_opensearch::operation::describe_domain::DescribeDomainError;
use aws_sdk_opensearch::types::DomainEndpointOptions;
use aws_sdk_sqs::error::SdkError;
use opensearch::http::Url;
use opensearch::http::response::Response;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use opensearch::{Error, OpenSearch as Client};
use serde_json::json;
use std::time::Duration;
use tokio::sync::OnceCell;
use tokio::time::sleep;
use tracing::debug;

pub const TEST_DOMAIN_NAME: &str = "test-domain";

/// A lazily-initialized, globally shared OpenSearch client for integration testing.
///
/// This `OnceCell` ensures that the client is only created once during the test lifecycle,
/// using the shared [`SdkConfig`] provided by [`get_aws_config()`].
static OPENSEARCH_CLIENT: OnceCell<Client> = OnceCell::const_new();

/// Returns a shared `opensearch::OpenSearch`-Client for interacting with LocalStack.
///
/// The client is initialized only once using a global `OnceCell`, and internally depends on
/// [`get_aws_config()`] for configuration (test credentials, region, LocalStack endpoint).
///
/// # Returns
///
/// A reference to a lazily-initialized `Client` instance.
pub async fn get_opensearch_client() -> &'static Client {
    let client = OPENSEARCH_CLIENT
        .get_or_init(|| async {
            let endpoint_url = Url::parse(&format!("http://localhost:4566/{TEST_DOMAIN_NAME}"))
                .expect("shouldn't fail parsing OpenSearch endpoint URL");
            let transport = TransportBuilder::new(SingleNodeConnectionPool::new(endpoint_url))
                .auth(
                    get_aws_config()
                        .await
                        .clone()
                        .try_into()
                        .expect("shouldn't fail extracting AWS-Config for OpenSearch"),
                )
                .service_name("es")
                .build()
                .expect("shouldn't fail creating OpenSearch-Transport");
            opensearch::OpenSearch::new(transport)
        })
        .await;
    debug!("Successfully initialized OpenSearch-Client.");
    client
}

/// Marker type representing the OpenSearch service in LocalStack-based tests.
///
/// Implements the `IntegrationTestService` trait to support lifecycle management
/// when used with the `#[localstack_test]` macro.
///
/// ### Dependencies
///
/// LocalStack requires **S3** to be activated when using OpenSearch.
/// You need to supply S3 manually with `#[localstack_test(services = [OpenSearch, S3])]`
pub struct OpenSearch();

#[async_trait]
impl IntegrationTestService for OpenSearch {
    const SERVICE_NAME: &'static str = "opensearch";

    async fn set_up() {
        set_up_domain()
            .await
            .expect("shouldn't fail creating OpenSearch-Domain");
        wait_until_domain_processed(TEST_DOMAIN_NAME)
            .await
            .expect("shouldn't fail waiting for domain  to complete processing");
        set_up_indices()
            .await
            .expect("shouldn't fail setting up indices");
    }
}

async fn set_up_domain() -> Result<CreateDomainOutput, SdkError<CreateDomainError>> {
    aws_sdk_opensearch::Client::new(get_aws_config().await)
        .create_domain()
        .domain_name(TEST_DOMAIN_NAME)
        .domain_endpoint_options(
            DomainEndpointOptions::builder()
                .custom_endpoint(format!("http://localhost:4566/{TEST_DOMAIN_NAME}"))
                .custom_endpoint_enabled(true)
                .build(),
        )
        .send()
        .await
}

async fn wait_until_domain_processed(
    domain: &'static str,
) -> Result<(), SdkError<DescribeDomainError, HttpResponse>> {
    let mut retries = 300;
    let mut processing = true;
    while processing {
        let res = aws_sdk_opensearch::Client::new(get_aws_config().await)
            .describe_domain()
            .domain_name(domain)
            .send()
            .await?;
        if res
            .clone()
            .domain_status
            .expect("shouldn't miss 'domain_status'")
            .processing
            .expect("shouldn't miss 'domain_status.processing'")
        {
            retries -= 1;
            debug!(
                remaining_retries = retries,
                domain = domain,
                "Domain is still being processed..."
            );
            if retries < 0 {
                return Err(SdkError::timeout_error("Domain took too long to process"));
            }
            sleep(Duration::from_millis(500)).await;
        } else {
            debug!(
                remaining_retries = retries,
                domain = domain,
                "Domain finished processing."
            );
            processing = false;
        }
    }
    Ok(())
}

async fn set_up_indices() -> Result<Response, Error> {
    let mapping = json!({
      "mappings": {
        "properties": {
          "itemId": {
            "type": "keyword"
          },
          "eventId": {
            "type": "keyword"
          },
          "shopId": {
            "type": "keyword"
          },
          "shopsItemId": {
            "type": "keyword"
          },
          "shopName": {
            "type": "keyword",
          },
          "titleDe": {
            "type": "search_as_you_type",
            "analyzer": "german"
          },
          "titleEn": {
            "type": "search_as_you_type",
            "analyzer": "english"
          },
          "descriptionDe": {
            "type": "text",
            "analyzer": "german",
          },
          "descriptionEn": {
            "type": "text",
            "analyzer": "english",
          },
          "price_eur": {
            "type": "unsigned_long",
          },
          "price_usd": {
            "type": "unsigned_long",
          },
          "price_gbp": {
            "type": "unsigned_long",
          },
          "price_aud": {
            "type": "unsigned_long",
          },
          "price_cad": {
            "type": "unsigned_long",
          },
          "price_nze": {
            "type": "unsigned_long",
          },
          "state": {
            "type": "keyword"
          },
          "isAvailable": {
            "type": "boolean"
          },
          "url": {
            "type": "keyword"
          },
          "images": {
            "type": "keyword"
          },
          "created": {
            "type": "date",
            "format": "strict_date_time"
          },
          "updated": {
            "type": "date",
            "format": "strict_date_time"
          }
        }
      }
    });

    get_opensearch_client()
        .await
        .indices()
        .create(opensearch::indices::IndicesCreateParts::Index("items"))
        .body(mapping)
        .send()
        .await
}

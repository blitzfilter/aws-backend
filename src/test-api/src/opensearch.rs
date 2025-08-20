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
use opensearch::params::Refresh;
use opensearch::{Error, GetParts, IndexParts, OpenSearch as Client};
use serde::de::DeserializeOwned;
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
    fn service_names(&self) -> &'static [&'static str] {
        &["opensearch", "s3"]
    }

    async fn set_up(&self) {
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

    async fn tear_down(&self) {
        // Clear all documents from the items index to ensure test isolation
        clear_index_data("items")
            .await
            .expect("shouldn't fail clearing OpenSearch index data");
        refresh_index("items").await;
        debug!("Cleared OpenSearch index data for test isolation");
    }
}

async fn set_up_domain() -> Result<CreateDomainOutput, SdkError<CreateDomainError>> {
    let client = aws_sdk_opensearch::Client::new(get_aws_config().await);
    
    // Check if domain already exists
    match client.describe_domain().domain_name(TEST_DOMAIN_NAME).send().await {
        Ok(_response) => {
            debug!("OpenSearch domain '{}' already exists, skipping creation", TEST_DOMAIN_NAME);
            // Return a fake response since the domain exists
            return Ok(CreateDomainOutput::builder().build());
        }
        Err(_) => {
            debug!("OpenSearch domain '{}' does not exist, creating it", TEST_DOMAIN_NAME);
        }
    }

    client
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
    use opensearch::indices::IndicesExistsParts;
    
    let client = get_opensearch_client().await;
    
    // Check if index already exists
    let exists_response = client
        .indices()
        .exists(IndicesExistsParts::Index(&["items"]))
        .send()
        .await?;
    
    if exists_response.status_code().is_success() {
        debug!("OpenSearch index 'items' already exists, skipping creation");
        // Return a mock response since index exists
        return Ok(exists_response);
    }
    
    debug!("OpenSearch index 'items' does not exist, creating it");
    
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
            "type": "text",
          },
          "titleDe": {
            "type": "text",
            "analyzer": "german"
          },
          "titleEn": {
            "type": "text",
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
          "priceEur": {
            "type": "unsigned_long",
          },
          "priceUsd": {
            "type": "unsigned_long",
          },
          "priceBbp": {
            "type": "unsigned_long",
          },
          "priceAud": {
            "type": "unsigned_long",
          },
          "priceCad": {
            "type": "unsigned_long",
          },
          "priceNze": {
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

    client
        .indices()
        .create(opensearch::indices::IndicesCreateParts::Index("items"))
        .body(mapping)
        .send()
        .await
}

pub async fn read_by_id<T: DeserializeOwned>(index: &str, id: impl Into<String>) -> T {
    let get_response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(index, &id.into()))
        .send()
        .await
        .unwrap();
    assert!(get_response.status_code().is_success());

    let response_doc: serde_json::Value = get_response.json().await.unwrap();
    serde_json::from_value(response_doc["_source"].clone()).unwrap()
}

pub async fn refresh_index(index: &str) {
    get_opensearch_client()
        .await
        .index(IndexParts::Index(index))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
}

/// Clears all documents from the specified OpenSearch index.
///
/// This function uses the delete-by-query API to remove all documents while
/// preserving the index structure and mappings.
async fn clear_index_data(index: &str) -> Result<Response, Error> {
    use opensearch::DeleteByQueryParts;
    use serde_json::json;

    let query = json!({
        "query": {
            "match_all": {}
        }
    });

    get_opensearch_client()
        .await
        .delete_by_query(DeleteByQueryParts::Index(&[index]))
        .body(query)
        .refresh(true)
        .send()
        .await
}

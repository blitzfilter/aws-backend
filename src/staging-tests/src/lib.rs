use aws_sdk_dynamodb::types::WriteRequest;
use aws_sdk_sqs::types::DeleteMessageBatchRequestEntry;
use opensearch::http::Url;
use opensearch::http::response::Response;
use opensearch::http::transport::{SingleNodeConnectionPool, TransportBuilder};
use serde::{Deserialize, Serialize};
pub use staging_tests_macros::staging_test;
use std::{collections::HashMap, error::Error, sync::OnceLock};
use tokio::sync::OnceCell;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CloudFormationOutput {
    pub api_gateway_endpoint_url: String,
    pub opensearch_item_domain_endpoint_url: String,
    pub dynamodb_table_1_name: String,
    pub item_write_new_queue_url: String,
    pub item_write_new_dead_letter_queue_url: String,
    pub item_write_update_queue_url: String,
    pub item_write_update_dead_letter_queue_url: String,
    pub item_materialize_dynamodb_new_queue_url: String,
    pub item_materialize_dynamodb_new_dead_letter_queue_url: String,
    pub item_materialize_dynamodb_update_queue_url: String,
    pub item_materialize_dynamodb_update_dead_letter_queue_url: String,
    pub item_materialize_opensearch_new_queue_url: String,
    pub item_materialize_opensearch_new_dead_letter_queue_url: String,
    pub item_materialize_opensearch_update_queue_url: String,
    pub item_materialize_opensearch_update_dead_letter_queue_url: String,
}

static CFN_OUTPUT: OnceLock<CloudFormationOutput> = OnceLock::new();
pub fn get_cfn_output() -> &'static CloudFormationOutput {
    CFN_OUTPUT.get_or_init(|| {
        let json = std::env::var("CFN_OUTPUT").expect("should have CFN_OUTPUT set in CI");
        serde_json::from_str::<CloudFormationOutput>(&json)
            .expect("shouldn't fail deserializing '$CFN_OUTPUT' to 'CloudFormationOutput'")
    })
}

static CONFIG: OnceCell<aws_config::SdkConfig> = OnceCell::const_new();
pub async fn get_aws_config() -> &'static aws_config::SdkConfig {
    CONFIG
        .get_or_init(|| async {
            let _ = tracing_subscriber::fmt()
                .json()
                .with_max_level(tracing::Level::INFO)
                .with_current_span(true)
                .with_ansi(false)
                .try_init();
            aws_config::defaults(aws_config::BehaviorVersion::latest())
                .load()
                .await
        })
        .await
}

static DYNAMODB_CLIENT: OnceCell<aws_sdk_dynamodb::Client> = OnceCell::const_new();
pub async fn get_dynamodb_client() -> &'static aws_sdk_dynamodb::Client {
    DYNAMODB_CLIENT
        .get_or_init(|| async { aws_sdk_dynamodb::Client::new(get_aws_config().await) })
        .await
}

static OPENSEARCH_CLIENT: OnceCell<opensearch::OpenSearch> = OnceCell::const_new();
pub async fn get_opensearch_client() -> &'static opensearch::OpenSearch {
    OPENSEARCH_CLIENT
        .get_or_init(|| async {
            let transport = TransportBuilder::new(SingleNodeConnectionPool::new(
                Url::parse(&get_cfn_output().opensearch_item_domain_endpoint_url)
                    .expect("shouldn't fail parsing 'opensearch_item_domain_endpoint_url' as URL"),
            ))
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
        .await
}

static SQS_CLIENT: OnceCell<aws_sdk_sqs::Client> = OnceCell::const_new();
pub async fn get_sqs_client() -> &'static aws_sdk_sqs::Client {
    SQS_CLIENT
        .get_or_init(|| async { aws_sdk_sqs::Client::new(get_aws_config().await) })
        .await
}

// Called inside the macro
pub async fn reset() {
    let cfn_output = get_cfn_output().clone();
    clear_ddb_table_data()
        .await
        .expect("shouldn't fail clearing table-data");
    clear_os_index_data("items")
        .await
        .expect("shouldn't fail clearing os-index 'items'");
    clear_qs(vec![
        cfn_output.item_write_new_queue_url,
        cfn_output.item_write_new_dead_letter_queue_url,
        cfn_output.item_write_update_queue_url,
        cfn_output.item_write_update_dead_letter_queue_url,
        cfn_output.item_materialize_dynamodb_new_queue_url,
        cfn_output.item_materialize_dynamodb_new_dead_letter_queue_url,
        cfn_output.item_materialize_dynamodb_update_queue_url,
        cfn_output.item_materialize_dynamodb_update_dead_letter_queue_url,
        cfn_output.item_materialize_opensearch_new_queue_url,
        cfn_output.item_materialize_opensearch_new_dead_letter_queue_url,
        cfn_output.item_materialize_opensearch_update_queue_url,
        cfn_output.item_materialize_opensearch_update_dead_letter_queue_url,
    ])
    .await
    .expect("shouldn't fail clearing queues");
}

/// Clears all items from the DynamoDB table to ensure test isolation.
///
/// This function scans the table and deletes all items in batches.
async fn clear_ddb_table_data() -> Result<(), Box<dyn Error>> {
    use aws_sdk_dynamodb::types::{AttributeValue, DeleteRequest};

    let client = get_dynamodb_client().await;

    // Scan the table to get all items
    let mut exclusive_start_key: Option<HashMap<String, AttributeValue>> = None;

    loop {
        let mut scan_request = client
            .scan()
            .table_name(get_cfn_output().dynamodb_table_1_name.clone());

        if let Some(start_key) = exclusive_start_key {
            scan_request = scan_request.set_exclusive_start_key(Some(start_key));
        }

        let scan_output = scan_request.consistent_read(true).send().await?;

        if let Some(items) = scan_output.items
            && !items.is_empty()
        {
            // Delete items in batches
            let delete_requests: Vec<WriteRequest> = items
                .into_iter()
                .map(|item| {
                    let mut key = HashMap::new();
                    key.insert("pk".to_string(), item.get("pk").unwrap().clone());
                    key.insert("sk".to_string(), item.get("sk").unwrap().clone());

                    WriteRequest::builder()
                        .delete_request(
                            DeleteRequest::builder().set_key(Some(key)).build().unwrap(),
                        )
                        .build()
                })
                .collect();

            // Process deletes in batches of 25 (DynamoDB limit)
            for chunk in delete_requests.chunks(25) {
                let mut request_items = HashMap::new();
                request_items.insert(
                    get_cfn_output().dynamodb_table_1_name.clone(),
                    chunk.to_vec(),
                );

                client
                    .batch_write_item()
                    .set_request_items(Some(request_items))
                    .send()
                    .await?;
                debug!("Cleared a chunk of size '{}' from table", chunk.len());
            }
        }

        // Check if there are more items to scan
        exclusive_start_key = scan_output.last_evaluated_key;
        if exclusive_start_key.is_none() {
            break;
        }
    }

    debug!(
        "Cleared table '{}'.",
        get_cfn_output().dynamodb_table_1_name
    );

    Ok(())
}

async fn clear_os_index_data(index: &str) -> Result<Response, opensearch::Error> {
    use opensearch::DeleteByQueryParts;
    use serde_json::json;

    let query = json!({
        "query": {
            "match_all": {}
        }
    });

    let res = get_opensearch_client()
        .await
        .delete_by_query(DeleteByQueryParts::Index(&[index]))
        .body(query)
        .refresh(true)
        .send()
        .await?;

    debug!("Cleared index '{index}'.");

    Ok(res)
}

// Manually deleting in batches as purging introduces 60s no-op window
async fn clear_q(queue_url: String) -> Result<(), Box<dyn Error>> {
    let client = get_sqs_client().await;
    loop {
        let resp = client
            .receive_message()
            .queue_url(queue_url.clone())
            .max_number_of_messages(10)
            .wait_time_seconds(1)
            .send()
            .await?;

        let messages = resp.messages.unwrap_or_default();
        if messages.is_empty() {
            break;
        }

        let entries: Vec<_> = messages
            .into_iter()
            .filter_map(|m| {
                m.receipt_handle.map(|handle| {
                    DeleteMessageBatchRequestEntry::builder()
                        .id(uuid::Uuid::new_v4().to_string())
                        .receipt_handle(handle)
                        .build()
                        .unwrap()
                })
            })
            .collect();

        client
            .delete_message_batch()
            .queue_url(queue_url.clone())
            .set_entries(Some(entries.clone()))
            .send()
            .await?;
        debug!(
            "Removed batch of size '{}' from queue '{}'.",
            entries.len(),
            queue_url
        );
    }

    debug!("Cleared queue '{queue_url}'.");

    Ok(())
}

async fn clear_qs(queue_urls: Vec<String>) -> Result<(), Box<dyn Error>> {
    for queue_url in queue_urls {
        clear_q(queue_url).await?;
    }
    Ok(())
}

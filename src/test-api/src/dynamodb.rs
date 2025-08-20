use std::collections::HashMap;

use crate::IntegrationTestService;
use crate::localstack::get_aws_config;
use async_trait::async_trait;
use aws_sdk_dynamodb::types::ScalarAttributeType::S;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, PutRequest, TableClass, WriteRequest,
};
use aws_sdk_dynamodb::{Client, Error};
use common::env::get_dynamodb_table_name;
use serde::Serialize;
use tokio::sync::OnceCell;
use tracing::debug;

/// A lazily-initialized, globally shared DynamoDB client for integration testing.
///
/// This `OnceCell` ensures that the client is only created once during the test lifecycle,
/// using the shared [`SdkConfig`] provided by [`get_aws_config()`].
static DYNAMODB_CLIENT: OnceCell<Client> = OnceCell::const_new();

/// Returns a shared `aws_sdk_dynamodb::Client` for interacting with LocalStack.
///
/// The client is initialized only once using a global `OnceCell`, and internally depends on
/// [`get_aws_config()`] for configuration (test credentials, region, LocalStack endpoint).
///
/// # Returns
///
/// A reference to a lazily-initialized `Client` instance.
pub async fn get_dynamodb_client() -> &'static Client {
    let client = DYNAMODB_CLIENT
        .get_or_init(|| async { Client::new(get_aws_config().await) })
        .await;
    debug!("Successfully initialized DynamoDB-Client.");
    client
}

/// Marker type representing the DynamoDB service in LocalStack-based tests.
///
/// Implements the [`IntegrationTestService`] trait to support lifecycle management
/// when used with the `#[localstack_test]` macro.
pub struct DynamoDB();

#[async_trait]
impl IntegrationTestService for DynamoDB {
    fn service_names(&self) -> &'static [&'static str] {
        &["dynamodb"]
    }

    async fn set_up(&self) {
        unsafe {
            std::env::set_var("DYNAMODB_TABLE_NAME", "table_1");
        }
        set_up_tables()
            .await
            .expect("shouldn't fail setting up tables");
    }

    async fn tear_down(&self) {
        // Clear all items from the table to ensure test isolation
        clear_table_data()
            .await
            .expect("shouldn't fail clearing DynamoDB table data");
        debug!("Cleared DynamoDB table data for test isolation");
    }
}

async fn set_up_tables() -> Result<(), Error> {
    set_up_table_items()
        .await
        .expect("shouldn't fail setting up table 'items'");

    debug!("Successfully set up tables.");

    Ok(())
}

async fn set_up_table_items() -> Result<(), Error> {
    let table_name = get_dynamodb_table_name();
    let client = get_dynamodb_client().await;
    
    // Check if table already exists
    match client.describe_table().table_name(table_name).send().await {
        Ok(_) => {
            debug!("Table '{}' already exists, skipping creation", table_name);
            return Ok(());
        }
        Err(_) => {
            debug!("Table '{}' does not exist, creating it", table_name);
        }
    }

    client
        .create_table()
        .table_name(table_name)
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(S)
                .build()?,
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("sk")
                .attribute_type(S)
                .build()?,
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("gsi_1_pk")
                .attribute_type(S)
                .build()?,
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("gsi_1_sk")
                .attribute_type(S)
                .build()?,
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()?,
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("sk")
                .key_type(KeyType::Range)
                .build()?,
        )
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("gsi_1")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("gsi_1_pk")
                        .key_type(KeyType::Hash)
                        .build()?,
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("gsi_1_sk")
                        .key_type(KeyType::Range)
                        .build()?,
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::Include)
                        .non_key_attributes("item_id")
                        .non_key_attributes("shop_id")
                        .non_key_attributes("shops_item_id")
                        .non_key_attributes("hash")
                        .build(),
                )
                .build()?,
        )
        .billing_mode(BillingMode::PayPerRequest)
        .table_class(TableClass::Standard)
        .send()
        .await?;

    Ok(())
}

pub fn mk_partial_put_batch_failure<T: Serialize>(
    table_name: &str,
    failures: Vec<T>,
) -> Option<HashMap<String, Vec<WriteRequest>>> {
    let put_failures = failures
        .into_iter()
        .map(serde_dynamo::to_item)
        .map(Result::unwrap)
        .map(|ddb_item| {
            PutRequest::builder()
                .set_item(Some(ddb_item))
                .build()
                .unwrap()
        })
        .map(|put_req| {
            WriteRequest::builder()
                .set_put_request(Some(put_req))
                .build()
        })
        .collect();
    Some(HashMap::from([(table_name.to_string(), put_failures)]))
}

/// Clears all items from the DynamoDB table to ensure test isolation.
///
/// This function scans the table and deletes all items in batches.
async fn clear_table_data() -> Result<(), Error> {
    use aws_sdk_dynamodb::types::{AttributeValue, DeleteRequest};

    let table_name = get_dynamodb_table_name();
    let client = get_dynamodb_client().await;

    // Scan the table to get all items
    let mut exclusive_start_key: Option<HashMap<String, AttributeValue>> = None;
    
    loop {
        let mut scan_request = client.scan().table_name(table_name);
        
        if let Some(start_key) = exclusive_start_key {
            scan_request = scan_request.set_exclusive_start_key(Some(start_key));
        }
        
        let scan_output = scan_request.send().await?;
        
        if let Some(items) = scan_output.items {
            if !items.is_empty() {
                // Delete items in batches
                let delete_requests: Vec<WriteRequest> = items
                    .into_iter()
                    .map(|item| {
                        let mut key = HashMap::new();
                        key.insert("pk".to_string(), item.get("pk").unwrap().clone());
                        key.insert("sk".to_string(), item.get("sk").unwrap().clone());
                        
                        WriteRequest::builder()
                            .delete_request(
                                DeleteRequest::builder()
                                    .set_key(Some(key))
                                    .build()
                                    .unwrap()
                            )
                            .build()
                    })
                    .collect();

                // Process deletes in batches of 25 (DynamoDB limit)
                for chunk in delete_requests.chunks(25) {
                    let mut request_items = HashMap::new();
                    request_items.insert(table_name.to_string(), chunk.to_vec());
                    
                    client
                        .batch_write_item()
                        .set_request_items(Some(request_items))
                        .send()
                        .await?;
                }
            }
        }
        
        // Check if there are more items to scan
        exclusive_start_key = scan_output.last_evaluated_key;
        if exclusive_start_key.is_none() {
            break;
        }
    }
    
    Ok(())
}

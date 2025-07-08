use crate::IntegrationTestService;
use crate::localstack::get_aws_config;
use async_trait::async_trait;
use aws_sdk_dynamodb::types::ScalarAttributeType::S;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, TableClass,
};
use aws_sdk_dynamodb::{Client, Error};
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
    const SERVICE_NAME: &'static str = "dynamodb";

    async fn set_up() {
        set_up_tables()
            .await
            .expect("shouldn't fail setting up tables");
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
    get_dynamodb_client()
        .await
        .create_table()
        .table_name("items")
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
                        .projection_type(ProjectionType::All)
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

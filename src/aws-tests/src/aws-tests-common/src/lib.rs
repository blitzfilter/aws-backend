use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

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

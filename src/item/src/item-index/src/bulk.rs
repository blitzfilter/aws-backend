use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct BulkResponse {
    pub took: u64,

    pub errors: bool,

    #[serde(default)]
    pub items: Vec<BulkItemResult>,
}

#[derive(Debug, Deserialize)]
pub struct BulkItemResult {
    #[serde(default)]
    pub update: Option<BulkOpResult>,

    #[serde(default)]
    pub create: Option<BulkOpResult>,
}

#[derive(Debug, Deserialize)]
pub struct BulkOpResult {
    #[serde(rename = "_index")]
    pub index: String,

    #[serde(rename = "_id")]
    pub id: String,

    #[serde(rename = "_version", default)]
    pub version: Option<u64>,

    pub result: String,

    pub status: u16,

    #[serde(default)]
    pub error: Option<BulkError>,
}

#[derive(Debug, Deserialize)]
pub struct BulkError {
    #[serde(rename = "type")]
    pub error_type: String,

    pub reason: String,

    #[serde(default)]
    pub index_uuid: Option<String>,

    #[serde(default)]
    pub shard: Option<String>,

    #[serde(default)]
    pub index: Option<String>,

    #[serde(flatten, default)]
    pub extra: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_parse_successful_response_when_no_errors_for_bulk_response() {
        let json = json!({
            "took": 30,
            "errors": false,
            "items": [
                {
                    "update": {
                        "_index": "items",
                        "_id": "1",
                        "_version": 2,
                        "result": "updated",
                        "status": 200
                    }
                }
            ]
        });

        let response: BulkResponse = serde_json::from_value(json).unwrap();

        assert_eq!(response.took, 30);
        assert!(!response.errors);
        assert_eq!(response.items.len(), 1);

        let update = response.items[0].update.as_ref().unwrap();
        assert_eq!(update.index, "items");
        assert_eq!(update.id, "1");
        assert_eq!(update.version, Some(2));
        assert_eq!(update.result, "updated");
        assert_eq!(update.status, 200);
        assert!(update.error.is_none());
    }

    #[test]
    fn should_parse_partial_failure_when_one_item_has_error_for_bulk_response() {
        let json = json!({
            "took": 12,
            "errors": true,
            "items": [
                {
                    "update": {
                        "_index": "items",
                        "_id": "2",
                        "_version": 3,
                        "result": "noop",
                        "status": 200
                    }
                },
                {
                    "update": {
                        "_index": "items",
                        "_id": "3",
                        "status": 409,
                        "result": "error",
                        "error": {
                            "type": "version_conflict_engine_exception",
                            "reason": "[items][3]: version conflict, document already exists",
                            "index": "items",
                            "shard": "shard-1",
                            "index_uuid": "uuid123"
                        }
                    }
                }
            ]
        });

        let response: BulkResponse = serde_json::from_value(json).unwrap();

        assert_eq!(response.took, 12);
        assert!(response.errors);
        assert_eq!(response.items.len(), 2);

        // Success item
        let update_ok = response.items[0].update.as_ref().unwrap();
        assert_eq!(update_ok.status, 200);
        assert!(update_ok.error.is_none());

        // Error item
        let update_err = response.items[1].update.as_ref().unwrap();
        assert_eq!(update_err.status, 409);
        assert!(update_err.error.is_some());

        let err = update_err.error.as_ref().unwrap();
        assert_eq!(err.error_type, "version_conflict_engine_exception");
        assert_eq!(
            err.reason,
            "[items][3]: version conflict, document already exists"
        );
        assert_eq!(err.index.as_deref(), Some("items"));
        assert_eq!(err.shard.as_deref(), Some("shard-1"));
        assert_eq!(err.index_uuid.as_deref(), Some("uuid123"));
    }

    #[test]
    fn should_default_to_empty_items_when_missing_for_bulk_response() {
        let json = json!({
            "took": 5,
            "errors": false
        });

        let response: BulkResponse = serde_json::from_value(json).unwrap();

        assert_eq!(response.took, 5);
        assert!(!response.errors);
        assert!(response.items.is_empty());
    }

    #[test]
    fn should_handle_missing_optional_fields_when_deserializing_bulk_error() {
        let json = json!({
            "type": "mapper_parsing_exception",
            "reason": "failed to parse field [price]",
            "some_extra_field": { "detail": "bad value" }
        });

        let err: BulkError = serde_json::from_value(json).unwrap();

        assert_eq!(err.error_type, "mapper_parsing_exception");
        assert_eq!(err.reason, "failed to parse field [price]");
        assert!(err.index_uuid.is_none());
        assert!(err.extra.is_some());
    }
}

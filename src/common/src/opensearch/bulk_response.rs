use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct BulkResponse {
    pub took: u64,

    pub errors: bool,

    #[serde(default)]
    pub items: Vec<BulkItemResult>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum BulkItemResult {
    Update { update: BulkOpResult },
    Create { create: BulkOpResult },
}

impl BulkItemResult {
    pub fn unwrap_update(self) -> BulkOpResult {
        match self {
            BulkItemResult::Update { update } => update,
            _ => panic!("Expected BulkItemResult::Update"),
        }
    }

    pub fn unwrap_create(self) -> BulkOpResult {
        match self {
            BulkItemResult::Create { create } => create,
            _ => panic!("Expected BulkItemResult::Create"),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct BulkOpResult {
    #[serde(rename = "_index")]
    pub index: String,

    #[serde(rename = "_id")]
    pub id: String,

    #[serde(rename = "_version", default)]
    pub version: Option<u64>,

    pub status: u16,

    #[serde(default)]
    pub error: Option<BulkError>,
}

impl BulkOpResult {
    pub fn is_err(&self) -> bool {
        self.error.is_some()
    }
}

#[derive(Debug, Deserialize, Clone)]
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
                        "status": 200
                    }
                }
            ]
        });

        let response: BulkResponse = serde_json::from_value(json).unwrap();

        assert_eq!(response.took, 30);
        assert!(!response.errors);
        assert_eq!(response.items.len(), 1);

        let update = response.items[0].clone().unwrap_update();
        assert_eq!(update.index, "items");
        assert_eq!(update.id, "1");
        assert_eq!(update.version, Some(2));
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
                        "status": 200
                    }
                },
                {
                    "update": {
                        "_index": "items",
                        "_id": "3",
                        "status": 409,
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
        let update_ok = response.items[0].clone().unwrap_update();
        assert_eq!(update_ok.status, 200);
        assert!(update_ok.error.is_none());

        // Error item
        let update_err = response.items[1].clone().unwrap_update();
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

    #[test]
    fn should_parse_successful_create_response_for_bulk_response() {
        let json = json!({
            "took": 15,
            "errors": false,
            "items": [
                {
                    "create": {
                        "_index": "items",
                        "_id": "10",
                        "_version": 1,
                        "status": 201
                    }
                }
            ]
        });

        let response: BulkResponse = serde_json::from_value(json).unwrap();

        assert_eq!(response.took, 15);
        assert!(!response.errors);
        assert_eq!(response.items.len(), 1);

        let create = match &response.items[0] {
            BulkItemResult::Create { create } => create,
            _ => panic!("Expected Create variant"),
        };

        assert_eq!(create.index, "items");
        assert_eq!(create.id, "10");
        assert_eq!(create.version, Some(1));
        assert_eq!(create.status, 201);
        assert!(create.error.is_none());
    }

    #[test]
    fn should_parse_failed_create_response_for_bulk_response() {
        let json = json!({
            "took": 8,
            "errors": true,
            "items": [
                {
                    "create": {
                        "_index": "items",
                        "_id": "11",
                        "status": 409,
                        "error": {
                            "type": "version_conflict_engine_exception",
                            "reason": "[items][11]: version conflict, document already exists",
                            "index": "items",
                            "shard": "shard-2",
                            "index_uuid": "uuid456"
                        }
                    }
                }
            ]
        });

        let response: BulkResponse = serde_json::from_value(json).unwrap();

        assert_eq!(response.took, 8);
        assert!(response.errors);
        assert_eq!(response.items.len(), 1);

        let create = match &response.items[0] {
            BulkItemResult::Create { create } => create,
            _ => panic!("Expected Create variant"),
        };

        assert_eq!(create.status, 409);
        assert!(create.error.is_some());

        let err = create.error.as_ref().unwrap();
        assert_eq!(err.error_type, "version_conflict_engine_exception");
        assert_eq!(
            err.reason,
            "[items][11]: version conflict, document already exists"
        );
        assert_eq!(err.index.as_deref(), Some("items"));
        assert_eq!(err.shard.as_deref(), Some("shard-2"));
        assert_eq!(err.index_uuid.as_deref(), Some("uuid456"));
    }

    #[test]
    fn should_parse_failed_update_response_for_bulk_response() {
        let json = json!({
          "errors": true,
          "items": [
            {
              "update": {
                "_id": "d5d619d3-676c-eab2-bf31-a3c1c106b4fb",
                "_index": "items",
                "error": {
                  "index": "items",
                  "index_uuid": "dcnQL_5lQDaKMdxVpD3E9Q",
                  "reason": "[d5d619d3-676c-eab2-bf31-a3c1c106b4fb]: document missing",
                  "shard": "1",
                  "type": "document_missing_exception"
                },
                "status": 404
              }
            }
          ],
          "took": 266
        });

        let response: BulkResponse = serde_json::from_value(json).unwrap();

        assert_eq!(response.took, 266);
        assert!(response.errors);
        assert_eq!(response.items.len(), 1);

        let update = match &response.items[0] {
            BulkItemResult::Update { update } => update,
            _ => panic!("Expected Update variant"),
        };

        assert_eq!(update.status, 404);
        assert!(update.error.is_some());

        let err = update.error.as_ref().unwrap();
        assert_eq!(err.error_type, "document_missing_exception");
        assert_eq!(
            err.reason,
            "[d5d619d3-676c-eab2-bf31-a3c1c106b4fb]: document missing"
        );
        assert_eq!(err.index.as_deref(), Some("items"));
        assert_eq!(err.shard.as_deref(), Some("1"));
        assert_eq!(err.index_uuid.as_deref(), Some("dcnQL_5lQDaKMdxVpD3E9Q"));
    }
}

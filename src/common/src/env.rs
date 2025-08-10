use std::{env, sync::OnceLock};

use tracing::warn;

static DYNAMODB_TABLE_NAME: OnceLock<String> = OnceLock::new();

pub fn get_dynamodb_table_name() -> &'static str {
    DYNAMODB_TABLE_NAME.get_or_init(|| {
        env::var("DYNAMODB_TABLE_NAME").unwrap_or_else(|err| {
            warn!(error = %err, "Attempted to read env-var 'DYNAMODB_TABLE_NAME' but failed. Defaulting to 'table_1'.");
            "table_1".to_string()
        })
    })
}

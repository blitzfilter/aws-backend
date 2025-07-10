use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::batch_write_item::{BatchWriteItemError, BatchWriteItemOutput};
use common::dynamodb_batch::DynamoDbBatch;
use item_core::item_event::record::ItemEventRecord;

#[async_trait]
pub trait WriteItemRecords {
    fn append_item_event_records(
        &self,
        event_records: DynamoDbBatch<ItemEventRecord>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;
}

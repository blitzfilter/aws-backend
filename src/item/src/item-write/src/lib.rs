use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::batch_write_item::{BatchWriteItemError, BatchWriteItemOutput};
use aws_sdk_dynamodb::operation::update_item::{UpdateItemError, UpdateItemOutput};
use common::dynamodb_batch::DynamoDbBatch;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::record::ItemRecord;
use item_core::item::update_record::ItemUpdateRecord;
use item_core::item_event::record::ItemEventRecord;

#[async_trait]
#[allow(clippy::result_large_err)]
pub trait WriteItemRecords {
    fn put_item_event_records(
        &self,
        item_event_records: DynamoDbBatch<ItemEventRecord>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;

    fn put_item_records(
        &self,
        item_records: DynamoDbBatch<ItemRecord>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;

    fn update_item_record(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        event_records: ItemUpdateRecord,
    ) -> Result<UpdateItemOutput, SdkError<UpdateItemError, HttpResponse>>;
}

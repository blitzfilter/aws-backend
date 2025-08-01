use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::batch_write_item::{BatchWriteItemError, BatchWriteItemOutput};
use aws_sdk_dynamodb::operation::update_item::{UpdateItemError, UpdateItemOutput};
use aws_sdk_dynamodb::types::AttributeValue;
use common::batch::Batch;
use common::has::Has;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::record::ItemRecord;
use item_core::item::update_record::ItemRecordUpdate;
use item_core::item_event::record::ItemEventRecord;
use std::collections::HashMap;

#[async_trait]
#[allow(clippy::result_large_err)]
pub trait WriteItemRecords {
    async fn put_item_event_records(
        &self,
        item_event_records: Batch<ItemEventRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;

    async fn put_item_records(
        &self,
        item_records: Batch<ItemRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>>;

    async fn update_item_record(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        event_records: ItemRecordUpdate,
    ) -> Result<UpdateItemOutput, SdkError<UpdateItemError, HttpResponse>>;
}

#[async_trait]
impl<T> WriteItemRecords for T
where
    T: Has<aws_sdk_dynamodb::Client> + Sync,
{
    async fn put_item_event_records(
        &self,
        item_event_records: Batch<ItemEventRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>> {
        self.get()
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(
                "items".to_owned(),
                item_event_records.into_dynamodb_write_requests(),
            )])))
            .send()
            .await
    }

    async fn put_item_records(
        &self,
        item_records: Batch<ItemRecord, 25>,
    ) -> Result<BatchWriteItemOutput, SdkError<BatchWriteItemError, HttpResponse>> {
        self.get()
            .batch_write_item()
            .set_request_items(Some(HashMap::from([(
                "items".to_owned(),
                item_records.into_dynamodb_write_requests(),
            )])))
            .send()
            .await
    }

    async fn update_item_record(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
        item_update_record: ItemRecordUpdate,
    ) -> Result<UpdateItemOutput, SdkError<UpdateItemError, HttpResponse>> {
        let pk = format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}");
        let sk = "item#materialized".to_string();

        let updates: HashMap<String, AttributeValue> =
            serde_dynamo::to_item(item_update_record).map_err(SdkError::construction_failure)?;

        let mut update_expressions = Vec::new();
        let mut expr_attr_names = HashMap::new();
        let mut expr_attr_values = HashMap::new();

        for (attr, val) in updates {
            let attr_placeholder = format!("#{attr}");
            let val_placeholder = format!(":{attr}_val");

            expr_attr_names.insert(attr_placeholder.clone(), attr.clone());
            expr_attr_values.insert(val_placeholder.clone(), val);
            update_expressions.push(format!("{attr_placeholder} = {val_placeholder}"));
        }

        let update_expr = format!("SET {}", update_expressions.join(", "));

        self.get()
            .update_item()
            .table_name("items")
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .update_expression(update_expr)
            .set_expression_attribute_names(Some(expr_attr_names))
            .set_expression_attribute_values(Some(expr_attr_values))
            .send()
            .await
    }
}

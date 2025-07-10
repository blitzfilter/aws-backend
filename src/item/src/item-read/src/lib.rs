use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::get_item::GetItemError;
use aws_sdk_dynamodb::operation::query::QueryError;
use aws_sdk_dynamodb::types::AttributeValue;
use common::has::Has;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::diff_record::ItemDiffRecord;
use item_core::item::record::ItemRecord;
use tracing::error;

#[async_trait]
pub trait ReadItemRecords {
    async fn get_item_record(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Option<ItemRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn query_item_diff_records(
        &self,
        shop_id: &ShopId,
        scan_index_forward: bool,
    ) -> Result<Vec<ItemDiffRecord>, SdkError<QueryError, HttpResponse>>;
}

#[async_trait]
impl<T> ReadItemRecords for T
where
    T: Has<aws_sdk_dynamodb::Client> + Sync,
{
    async fn get_item_record(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Option<ItemRecord>, SdkError<GetItemError, HttpResponse>> {
        let pk = format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}");
        let sk = "item#materialized".to_string();
        let rec = self
            .get()
            .get_item()
            .table_name("items")
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, ItemRecord>)
            .and_then(|item_record_res| match item_record_res {
                Ok(item_record) => Some(item_record),
                Err(err) => {
                    error!(error = %err, "Failed deserializing ItemRecord.");
                    None
                }
            });

        Ok(rec)
    }

    async fn query_item_diff_records(
        &self,
        shop_id: &ShopId,
        scan_index_forward: bool,
    ) -> Result<Vec<ItemDiffRecord>, SdkError<QueryError, HttpResponse>> {
        let records = self
            .get()
            .query()
            .table_name("items")
            .index_name("gsi_1")
            .key_condition_expression("#gsi_1_pk = :gsi_1_pk_val")
            .expression_attribute_names("#gsi_1_pk", "gsi_1_pk")
            .expression_attribute_values(":gsi_1_pk_val", AttributeValue::S(shop_id.to_string()))
            .scan_index_forward(scan_index_forward)
            .projection_expression(
                "#item_id, #shop_id, #shops_item_id, #price_currency, #price_amount, #state, #url, #hash"
            )
            .expression_attribute_names("#item_id", "item_id")
            .expression_attribute_names("#shop_id", "shop_id")
            .expression_attribute_names("#shops_item_id", "shops_item_id")
            .expression_attribute_names("#price_currency", "price_currency")
            .expression_attribute_names("#price_amount", "price_amount")
            .expression_attribute_names("#state", "state")
            .expression_attribute_names("#url", "url")
            .expression_attribute_names("#hash", "hash")
            .expression_attribute_names("#gsi_1_pk", "gsi_1_pk")
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|qo| qo.items.unwrap_or_default())
            .map(serde_dynamo::from_item::<_, ItemDiffRecord>)
            .filter_map(|result| match result {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(error = %err, "Failed deserializing ItemDiffRecord.");
                    None
                }
            })
            .collect();

        Ok(records)
    }
}

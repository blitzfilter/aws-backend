use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError;
use aws_sdk_dynamodb::operation::get_item::GetItemError;
use aws_sdk_dynamodb::operation::query::QueryError;
use aws_sdk_dynamodb::types::{AttributeValue, KeysAndAttributes};
use common::batch::Batch;
use common::batch::dynamodb::BatchGetItemResult;
use common::env::get_dynamodb_table_name;
use common::item_id::ItemKey;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item::hash::ItemSummaryHash;
use item_core::item::record::ItemRecord;
use std::collections::HashMap;
use tracing::error;

#[async_trait]
#[mockall::automock]
pub trait QueryItemRepository {
    async fn get_item_record(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Option<ItemRecord>, SdkError<GetItemError, HttpResponse>>;

    async fn get_item_records(
        &self,
        item_keys: &Batch<ItemKey, 100>,
    ) -> Result<BatchGetItemResult<ItemRecord, ItemKey>, SdkError<BatchGetItemError, HttpResponse>>;

    async fn exist_item_records(
        &self,
        item_keys: &Batch<ItemKey, 100>,
    ) -> Result<BatchGetItemResult<ItemKey, ItemKey>, SdkError<BatchGetItemError, HttpResponse>>;

    async fn query_item_hashes(
        &self,
        shop_id: &ShopId,
        scan_index_forward: bool,
    ) -> Result<Vec<ItemSummaryHash>, SdkError<QueryError, HttpResponse>>;
}

#[derive(Debug)]
pub struct QueryItemRepositoryImpl<'a> {
    client: &'a Client,
}

impl<'a> QueryItemRepositoryImpl<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl<'a> QueryItemRepository for QueryItemRepositoryImpl<'a> {
    async fn get_item_record(
        &self,
        shop_id: &ShopId,
        shops_item_id: &ShopsItemId,
    ) -> Result<Option<ItemRecord>, SdkError<GetItemError, HttpResponse>> {
        let rec = self
            .client
            .get_item()
            .table_name(get_dynamodb_table_name())
            .key("pk", AttributeValue::S(mk_pk(shop_id, shops_item_id)))
            .key("sk", AttributeValue::S(mk_sk().to_owned()))
            .send()
            .await?
            .item
            .map(serde_dynamo::from_item::<_, ItemRecord>)
            .and_then(|item_record_res| match item_record_res {
                Ok(item_record) => Some(item_record),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ItemRecord>(), "Failed deserializing ItemRecord.");
                    None
                }
            });

        Ok(rec)
    }

    async fn get_item_records(
        &self,
        item_keys: &Batch<ItemKey, 100>,
    ) -> Result<BatchGetItemResult<ItemRecord, ItemKey>, SdkError<BatchGetItemError, HttpResponse>>
    {
        let keys = item_keys
            .iter()
            .map(|item_key| {
                let mut columns = HashMap::with_capacity(2);
                columns.insert(
                    "pk".to_owned(),
                    AttributeValue::S(mk_pk(&item_key.shop_id, &item_key.shops_item_id)),
                );
                columns.insert("sk".to_owned(), AttributeValue::S(mk_sk().to_owned()));
                columns
            })
            .collect();
        let keys_and_attributes = KeysAndAttributes::builder()
            .set_keys(Some(keys))
            .build()
            .expect("shouldn't fail because we previously set the only required field 'keys'.");
        let request_items = Some(HashMap::from([(
            get_dynamodb_table_name().to_owned(),
            keys_and_attributes,
        )]));
        let response = self
            .client
            .batch_get_item()
            .set_request_items(request_items)
            .send()
            .await?;

        let records = response
            .responses
            .unwrap_or_default()
            .remove(get_dynamodb_table_name())
            .unwrap_or_default()
            .into_iter()
            .map(serde_dynamo::from_item::<_, ItemRecord>)
            .filter_map(|result| match result {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ItemRecord>(), "Failed deserializing ItemRecord.");
                    None
                }
            })
            .collect::<Vec<_>>();

        let unprocessed = response
            .unprocessed_keys
            .unwrap_or_default()
            .remove(get_dynamodb_table_name())
            .map(|keys_and_attributes| keys_and_attributes.keys)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|attr_map| match extract_item_key(attr_map) {
                Ok(key) => Some(key),
                Err(err) => {
                    error!(
                        error = err,
                        "Failed extracting ItemKey from BatchGetItemOutput::unprocessed_keys."
                    );
                    None
                }
            })
            .collect::<Vec<_>>();

        let batch_result = BatchGetItemResult {
            items: records,
            unprocessed: if unprocessed.is_empty() {
                None
            } else {
                Some(Batch::try_from(unprocessed).expect(
                    "shouldn't fail creating batch because DynamoDB cannot respond \
                                with more failed ItemKeys than those requested.",
                ))
            },
        };
        Ok(batch_result)
    }

    async fn exist_item_records(
        &self,
        item_keys: &Batch<ItemKey, 100>,
    ) -> Result<BatchGetItemResult<ItemKey, ItemKey>, SdkError<BatchGetItemError, HttpResponse>>
    {
        let keys = item_keys
            .iter()
            .map(|item_key| {
                let mut columns = HashMap::with_capacity(2);
                columns.insert(
                    "pk".to_owned(),
                    AttributeValue::S(mk_pk(&item_key.shop_id, &item_key.shops_item_id)),
                );
                columns.insert("sk".to_owned(), AttributeValue::S(mk_sk().to_owned()));
                columns
            })
            .collect();
        let keys_and_attributes = KeysAndAttributes::builder()
            .set_keys(Some(keys))
            .projection_expression("pk")
            .build()
            .expect("shouldn't fail because we previously set the only required field 'keys'.");
        let request_items = Some(HashMap::from([(
            get_dynamodb_table_name().to_owned(),
            keys_and_attributes,
        )]));
        let response = self
            .client
            .batch_get_item()
            .set_request_items(request_items)
            .send()
            .await?;

        let records = response
            .responses
            .unwrap_or_default()
            .remove(get_dynamodb_table_name())
            .unwrap_or_default()
            .into_iter()
            .map(extract_item_key)
            .filter_map(|result| match result {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(error = %err, "Failed extracting ItemKey.");
                    None
                }
            })
            .collect::<Vec<_>>();

        let unprocessed = response
            .unprocessed_keys
            .unwrap_or_default()
            .remove(get_dynamodb_table_name())
            .map(|keys_and_attributes| keys_and_attributes.keys)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|attr_map| match extract_item_key(attr_map) {
                Ok(key) => Some(key),
                Err(err) => {
                    error!(
                        error = err,
                        "Failed extracting ItemKey from BatchGetItemOutput::unprocessed_keys."
                    );
                    None
                }
            })
            .collect::<Vec<_>>();

        let batch_result = BatchGetItemResult {
            items: records,
            unprocessed: if unprocessed.is_empty() {
                None
            } else {
                Some(Batch::try_from(unprocessed).expect(
                    "shouldn't fail creating batch because DynamoDB cannot respond \
                                with more failed ItemKeys than those requested.",
                ))
            },
        };
        Ok(batch_result)
    }

    async fn query_item_hashes(
        &self,
        shop_id: &ShopId,
        scan_index_forward: bool,
    ) -> Result<Vec<ItemSummaryHash>, SdkError<QueryError, HttpResponse>> {
        let records = self
            .client
            .query()
            .table_name(get_dynamodb_table_name())
            .index_name("gsi_1")
            .key_condition_expression("#gsi_1_pk = :gsi_1_pk_val")
            .expression_attribute_names("#gsi_1_pk", "gsi_1_pk")
            .expression_attribute_values(
                ":gsi_1_pk_val",
                AttributeValue::S(format!("shop_id#{shop_id}")),
            )
            .scan_index_forward(scan_index_forward)
            .expression_attribute_names("#gsi_1_pk", "gsi_1_pk")
            .into_paginator()
            .send()
            .try_collect()
            .await?
            .into_iter()
            .flat_map(|qo| qo.items.unwrap_or_default())
            .map(serde_dynamo::from_item::<_, ItemSummaryHash>)
            .filter_map(|result| match result {
                Ok(event) => Some(event),
                Err(err) => {
                    error!(error = %err, type = %std::any::type_name::<ItemSummaryHash>(), "Failed deserializing ItemSummaryHash.");
                    None
                }
            })
            .collect();

        Ok(records)
    }
}

fn mk_pk(shop_id: &ShopId, shops_item_id: &ShopsItemId) -> String {
    format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}")
}

fn mk_sk() -> &'static str {
    "item#materialized"
}

fn extract_item_key(map: HashMap<String, AttributeValue>) -> Result<ItemKey, String> {
    let mut map = map;

    // ugly af but much more efficient due to slices than using iterators in functional-style here
    if let Some(pk_attr) = map.remove("pk") {
        let pk_res = pk_attr.as_s();
        if let Ok(pk) = pk_res {
            if let Some((shop_id, shops_item_id)) = pk
                .trim_start_matches("item#shop_id#")
                .split_once("#shops_item_id#")
            {
                Ok(ItemKey {
                    shop_id: shop_id.into(),
                    shops_item_id: shops_item_id.into(),
                })
            } else {
                Err(format!("Parsing pk '{pk}' failed."))
            }
        } else {
            Err(format!("Extracted value for pk '{pk_attr:?}' failed."))
        }
    } else {
        Err(format!(
            "AttributeValue-Map does not contain key pk: '{map:?}'."
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::repository::extract_item_key;
    use aws_sdk_dynamodb::types::AttributeValue;
    use common::item_id::ItemKey;
    use std::collections::HashMap;

    #[rstest::rstest]
    #[case::differing("abcdefg", "123456")]
    #[case::identical("abcdefg", "abcdefg")]
    #[case::containing_separator("abcdefg#boop", "1874874")]
    fn should_extract_item_key_from_pk_sk_map_when_pk_exists_and_is_valid_for(
        #[case] shop_id: &str,
        #[case] shops_item_id: &str,
    ) {
        let map = HashMap::from([(
            "pk".to_owned(),
            AttributeValue::S(format!(
                "item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"
            )),
        )]);
        let expected = ItemKey {
            shop_id: shop_id.into(),
            shops_item_id: shops_item_id.into(),
        };

        let actual = extract_item_key(map);

        assert!(actual.is_ok());
        assert_eq!(expected, actual.unwrap());
    }
}

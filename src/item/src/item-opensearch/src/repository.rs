use crate::item_document::ItemDocument;
use crate::item_state_document::ItemStateDocument;
use crate::item_update_document::ItemUpdateDocument;
use async_trait::async_trait;
use common::currency::domain::Currency;
use common::item_id::ItemId;
use common::item_state::domain::ItemState;
use common::language::domain::Language;
use common::opensearch::{bulk_response::BulkResponse, search_response::SearchResponse};
use common::page::Page;
use common::sort::{Sort, SortDirection};
use item_core::item::SortItemField;
use opensearch::{BulkOperation, BulkOperations, BulkParts, SearchParts};
use search_filter_core::search_filter::SearchFilter;
use serde::ser::Error;
use serde_json::json;
use std::collections::HashMap;
use std::ops::Deref;
use time::format_description::well_known;

#[async_trait]
#[mockall::automock]
pub trait ItemOpenSearchRepository {
    async fn create_item_documents(
        &self,
        documents: Vec<ItemDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn update_item_documents(
        &self,
        updates: HashMap<ItemId, ItemUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn search_item_documents(
        &self,
        search_filter: &SearchFilter,
        language: &Language,
        currency: &Currency,
        sort: &Option<Sort<SortItemField>>,
        page: &Option<Page>,
    ) -> Result<SearchResponse<ItemDocument>, opensearch::Error>;
}

pub struct ItemOpenSearchRepositoryImpl<'a> {
    client: &'a opensearch::OpenSearch,
}

impl<'a> ItemOpenSearchRepositoryImpl<'a> {
    pub fn new(client: &'a opensearch::OpenSearch) -> Self {
        ItemOpenSearchRepositoryImpl { client }
    }
}

#[async_trait]
impl<'a> ItemOpenSearchRepository for ItemOpenSearchRepositoryImpl<'a> {
    async fn create_item_documents(
        &self,
        documents: Vec<ItemDocument>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();

        for doc in documents {
            ops.push(BulkOperation::create(doc._id(), &doc))?;
        }

        self.client
            .bulk(BulkParts::Index("items"))
            .body(vec![ops])
            .send()
            .await?
            .json::<BulkResponse>()
            .await
    }

    async fn update_item_documents(
        &self,
        updates: HashMap<ItemId, ItemUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();
        for (_id, doc) in updates {
            ops.push(BulkOperation::update(
                _id,
                json!({
                "doc": doc
                }),
            ))?;
        }

        self.client
            .bulk(BulkParts::Index("items"))
            .body(vec![ops])
            .send()
            .await?
            .json::<BulkResponse>()
            .await
    }

    async fn search_item_documents(
        &self,
        search_filter: &SearchFilter,
        language: &Language,
        currency: &Currency,
        sort: &Option<Sort<SortItemField>>,
        page: &Option<Page>,
    ) -> Result<SearchResponse<ItemDocument>, opensearch::Error> {
        let mut must = vec![];

        let (title_field, description_field) = match language {
            Language::De => ("titleDe", "descriptionDe"),
            Language::En => ("titleEn", "descriptionEn"),
            _ => ("titleDe", "descriptionDe"),
        };
        must.push(json!({
            "multi_match": {
                "query": search_filter.item_query.as_ref(),
                "fields": [
                    format!("{title_field}^3"),
                    format!("{description_field}^1"),
                ],
                "fuzziness": "AUTO",
                "operator": "and"
            }
        }));

        if let Some(shop_name_query) = &search_filter.shop_name_query {
            must.push(json!({
                "match": {
                    "shopName": {
                        "query": shop_name_query.deref(),
                        "fuzziness": "AUTO",
                        "operator": "and"
                    }
                }
            }));
        }

        match search_filter
            .state_query
            .0
            .iter()
            .collect::<Vec<&ItemState>>()
            .as_slice()
        {
            [] => {}
            [ItemState::Available] => {
                must.push(json!({
                    "term": { "isAvailable": true }
                }));
            }
            states => {
                let state_values: Vec<&str> = states
                    .iter()
                    .map(|state| ItemStateDocument::from(**state))
                    .map(|s| s.as_str())
                    .collect();

                must.push(json!({
                    "terms": { "state": state_values }
                }));
            }
        }

        let price_field = match currency {
            Currency::Eur => "priceEur",
            Currency::Gbp => "priceGbp",
            Currency::Usd => "priceUsd",
            Currency::Aud => "priceAud",
            Currency::Cad => "priceCad",
            Currency::Nzd => "priceNzd",
        };
        if let Some(min) = search_filter
            .price_query
            .and_then(|price_query| price_query.min)
        {
            must.push(json!({
                "range": { price_field: { "gte": min.deref() } }
            }));
        }
        if let Some(max) = search_filter
            .price_query
            .and_then(|price_query| price_query.max)
        {
            must.push(json!({
                "range": { price_field: { "lte": max.deref() } }
            }));
        }

        if let Some(min) = search_filter
            .created_query
            .and_then(|created_query| created_query.min)
        {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            must.push(json!({
                "range": { "created": { "gte": formatted_min } }
            }));
        }
        if let Some(max) = search_filter
            .created_query
            .and_then(|created_query| created_query.max)
        {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            must.push(json!({
                "range": { "created": { "lte": formatted_max } }
            }));
        }

        if let Some(min) = search_filter
            .updated_query
            .and_then(|updated_query| updated_query.min)
        {
            let formatted_min = min
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            must.push(json!({
                "range": { "updated": { "gte": formatted_min } }
            }));
        }
        if let Some(max) = search_filter
            .updated_query
            .and_then(|updated_query| updated_query.max)
        {
            let formatted_max = max
                .format(&well_known::Rfc3339)
                .map_err(serde_json::Error::custom)?;
            must.push(json!({
                "range": { "updated": { "lte": formatted_max } }
            }));
        }

        let mut body = json!({
            "query": {
                "bool": { "must": must }
            }
        });

        if let Some(p) = page {
            body.as_object_mut()
                .unwrap()
                .insert("from".to_string(), json!(p.from));
            body.as_object_mut()
                .unwrap()
                .insert("size".to_string(), json!(p.size));
        }

        if let Some(sort) = sort {
            let sort_field = match sort.field {
                SortItemField::Price => price_field,
                SortItemField::Created => "created",
                SortItemField::Updated => "updated",
            };
            let order = match sort.direction {
                SortDirection::Asc => "asc",
                SortDirection::Desc => "desc",
            };
            body.as_object_mut().unwrap().insert(
                "sort".to_string(),
                json!([
                    { sort_field: { "order": order, "missing": "_last", } },
                    { "itemId": { "order": "asc"} }
                ]),
            );
        }

        let response = self
            .client
            .search(SearchParts::Index(&["items"]))
            .body(body)
            .send()
            .await?;
        let search_response = response.json::<SearchResponse<ItemDocument>>().await?;

        Ok(search_response)
    }
}

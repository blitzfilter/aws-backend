use crate::bulk_response::BulkResponse;
use crate::item_document::ItemDocument;
use crate::item_update_document::ItemUpdateDocument;
use async_trait::async_trait;
use common::item_id::ItemId;
use opensearch::{BulkOperation, BulkOperations, BulkParts};
use serde_json::json;
use std::collections::HashMap;

#[async_trait]
pub trait ItemOpenSearchRepository {
    async fn create_item_documents(
        &self,
        documents: Vec<ItemDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn update_item_documents(
        &self,
        updates: HashMap<ItemId, ItemUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;
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
}

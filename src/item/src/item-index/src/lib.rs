pub mod bulk;

use crate::bulk::BulkResponse;
use async_trait::async_trait;
use common::has::Has;
use common::item_id::ItemId;
use item_core::item::document::ItemDocument;
use item_core::item::update_document::ItemUpdateDocument;
use opensearch::{BulkOperation, BulkOperations, BulkParts};
use serde_json::json;
use std::collections::HashMap;

#[async_trait]
pub trait IndexItemDocuments {
    async fn create_item_documents(
        &self,
        updates: Vec<ItemDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;

    async fn update_item_documents(
        &self,
        updates: HashMap<ItemId, ItemUpdateDocument>,
    ) -> Result<BulkResponse, opensearch::Error>;
}

#[async_trait]
impl<T: Has<opensearch::OpenSearch> + Sync> IndexItemDocuments for T {
    async fn create_item_documents(
        &self,
        documents: Vec<ItemDocument>,
    ) -> Result<BulkResponse, opensearch::Error> {
        let mut ops = BulkOperations::new();

        for doc in documents {
            ops.push(BulkOperation::create(doc._id(), &doc))?;
        }

        self.get()
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

        self.get()
            .bulk(BulkParts::Index("items"))
            .body(vec![ops])
            .send()
            .await?
            .json::<BulkResponse>()
            .await
    }
}

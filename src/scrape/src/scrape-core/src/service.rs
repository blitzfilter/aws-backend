use crate::data::{ScrapeItem, ScrapeItemChangeCommandData};
use async_trait::async_trait;
use aws_sdk_dynamodb::operation::query::QueryError;
use aws_sdk_sqs::error::SdkError;
use common::batch::Batch;
use common::shop_id::ShopId;
use item_read::repository::ReadItemRecords;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PublishScrapeItemsContext {
    pub dynamodb_client: aws_sdk_dynamodb::Client,
    pub sqs_client: aws_sdk_sqs::Client,
    pub sqs_url: String,
}

#[async_trait]
pub trait PublishScrapeItems {
    async fn extract_changed_items(
        &self,
        scrape_items: impl Iterator<Item = ScrapeItem> + Send,
        shop_id: &ShopId,
    ) -> Result<impl Iterator<Item = ScrapeItemChangeCommandData>, SdkError<QueryError>>;

    async fn publish_scrape_items(&self, scrape_items: Batch<ScrapeItem, 10>);
}

#[async_trait]
impl PublishScrapeItems for PublishScrapeItemsContext {
    async fn extract_changed_items(
        &self,
        scrape_items: impl Iterator<Item = ScrapeItem> + Send,
        shop_id: &ShopId,
    ) -> Result<impl Iterator<Item = ScrapeItemChangeCommandData>, SdkError<QueryError>> {
        let shop_universe = self
            .dynamodb_client
            .query_item_hashes(shop_id, true)
            .await?
            .into_iter()
            .map(|item_summary_hash| (item_summary_hash.shops_item_id, item_summary_hash.hash))
            .collect::<HashMap<_, _>>();

        let it =
            scrape_items.filter_map(move |scrape_item| scrape_item.into_changes(&shop_universe));
        Ok(it)
    }

    async fn publish_scrape_items(&self, _scrape_items: Batch<ScrapeItem, 10>) {
        todo!();
    }
}

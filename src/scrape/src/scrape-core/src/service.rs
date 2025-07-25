use crate::data::ScrapeItem;
use async_trait::async_trait;
use common::batch::Batch;

#[derive(Debug, Clone)]
pub struct PublishScrapeItemsContext {
    pub dynamodb_client: aws_sdk_dynamodb::Client,
    pub sqs_client: aws_sdk_sqs::Client,
    pub sqs_url: String,
}

#[async_trait]
pub trait PublishScrapeItems {
    async fn publish_scrape_items(&self, scrape_items: Batch<ScrapeItem, 10>);
}

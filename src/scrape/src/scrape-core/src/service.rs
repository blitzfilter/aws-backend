use crate::data::{ScrapeItem, ScrapeItemChangeCommandData};
use async_trait::async_trait;
use aws_sdk_dynamodb::operation::query::QueryError;
use aws_sdk_sqs::config::http::HttpResponse;
use aws_sdk_sqs::error::SdkError;
use aws_sdk_sqs::operation::send_message_batch::{SendMessageBatchError, SendMessageBatchOutput};
use common::batch::Batch;
use common::has::HasKey;
use common::shop_id::ShopId;
use item_core::item::command_data::{CreateItemCommandData, UpdateItemCommandData};
use item_read::repository::ReadItemRecords;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct PublishScrapeItemsContext {
    pub dynamodb_client: aws_sdk_dynamodb::Client,
    pub sqs_client: aws_sdk_sqs::Client,
    pub sqs_create_url: String,
    pub sqs_update_url: String,
}

#[async_trait]
pub trait PublishScrapeItems {
    async fn filter_changed_items(
        &self,
        scrape_items: impl Iterator<Item = ScrapeItem> + Send,
        shop_id: &ShopId,
    ) -> Result<impl Iterator<Item = ScrapeItemChangeCommandData>, SdkError<QueryError>>;

    async fn publish_scrape_items_create(
        &self,
        scrape_items: Batch<CreateItemCommandData, 10>,
    ) -> Result<SendMessageBatchOutput, SdkError<SendMessageBatchError, HttpResponse>>;

    async fn publish_scrape_items_update(
        &self,
        scrape_items: Batch<UpdateItemCommandData, 10>,
    ) -> Result<SendMessageBatchOutput, SdkError<SendMessageBatchError, HttpResponse>>;
}

#[async_trait]
impl PublishScrapeItems for PublishScrapeItemsContext {
    async fn filter_changed_items(
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

    async fn publish_scrape_items_create(
        &self,
        scrape_items: Batch<CreateItemCommandData, 10>,
    ) -> Result<SendMessageBatchOutput, SdkError<SendMessageBatchError, HttpResponse>> {
        self.publish_scrape_items(&self.sqs_create_url, scrape_items)
            .await
    }

    async fn publish_scrape_items_update(
        &self,
        scrape_items: Batch<UpdateItemCommandData, 10>,
    ) -> Result<SendMessageBatchOutput, SdkError<SendMessageBatchError, HttpResponse>> {
        self.publish_scrape_items(&self.sqs_update_url, scrape_items)
            .await
    }
}

impl PublishScrapeItemsContext {
    async fn publish_scrape_items<T>(
        &self,
        queue_url: &str,
        scrape_items: Batch<T, 10>,
    ) -> Result<SendMessageBatchOutput, SdkError<SendMessageBatchError, HttpResponse>>
    where
        T: Serialize + HasKey,
        T::Key: Into<String>,
    {
        self.sqs_client
            .send_message_batch()
            .set_entries(Some(scrape_items.into_sqs_message_entries()))
            .queue_url(queue_url)
            .send()
            .await
    }

    pub async fn filter_and_publish_scrape_items<T: IntoIterator<Item = ScrapeItem> + Send>(
        &self,
        scrape_items: T,
        shop_id: &ShopId,
    ) -> Result<(), T> {
        Err(scrape_items)
    }
}

use crate::data::{ScrapeItem, ScrapeItemChangeCommandData};
use async_trait::async_trait;
use aws_sdk_dynamodb::operation::query::QueryError;
use aws_sdk_sqs::config::http::HttpResponse;
use aws_sdk_sqs::error::SdkError;
use aws_sdk_sqs::operation::send_message_batch::{SendMessageBatchError, SendMessageBatchOutput};
use common::batch::Batch;
use common::has::HasKey;
use common::item_id::ItemKey;
use common::shop_id::ShopId;
use item_read::repository::ReadItemRecords;
use serde::Serialize;
use std::collections::HashMap;
use tracing::error;

#[derive(Debug, Clone)]
pub struct PublishScrapeItemsContext<'a> {
    pub dynamodb_client: &'a aws_sdk_dynamodb::Client,
    pub sqs_client: &'a aws_sdk_sqs::Client,
    pub sqs_create_url: String,
    pub sqs_update_url: String,
}

#[async_trait]
pub trait PublishScrapeItems {
    async fn publish_scrape_items(
        &self,
        scrape_items: impl IntoIterator<Item = ScrapeItem> + Send,
    ) -> Result<(), impl IntoIterator<Item = ItemKey>>;
}

#[async_trait]
impl<'a> PublishScrapeItems for PublishScrapeItemsContext<'a> {
    async fn publish_scrape_items(
        &self,
        scrape_items: impl IntoIterator<Item = ScrapeItem> + Send,
    ) -> Result<(), impl IntoIterator<Item = ItemKey>> {
        let mut failures = Vec::new();
        let mut assessed_create = Vec::new();
        let mut assessed_update = Vec::new();
        let grouped: HashMap<ShopId, Vec<ScrapeItem>> =
            scrape_items
                .into_iter()
                .fold(HashMap::new(), |mut acc, item| {
                    acc.entry(item.shop_id.clone()).or_default().push(item);
                    acc
                });

        for (shop_id, items) in grouped {
            let keys = items.iter().map(|item| item.key()).collect::<Vec<_>>();
            let assessed_res = self.assess(items.into_iter(), &shop_id).await;
            match assessed_res {
                Ok(assessed_items) => {
                    assessed_items.into_iter().for_each(|item| match item {
                        ScrapeItemChangeCommandData::Create(item) => assessed_create.push(item),
                        ScrapeItemChangeCommandData::Update(item) => assessed_update.push(item),
                    });
                }
                Err(err) => {
                    error!(error = %err, shopId = %shop_id, "Failed assessing ScrapeItems.");
                    failures.extend(keys);
                }
            }
        }

        for batch_create in Batch::<_, 10>::chunked_from(assessed_create.into_iter()) {
            let keys = batch_create
                .iter()
                .map(|item| item.key())
                .collect::<Vec<_>>();
            let send_msg_batch_res = self.publish(&self.sqs_create_url, batch_create).await;
            handle_message_batch_result(send_msg_batch_res, keys, &mut failures);
        }

        for batch_update in Batch::<_, 10>::chunked_from(assessed_update.into_iter()) {
            let keys = batch_update
                .iter()
                .map(|item| item.key())
                .collect::<Vec<_>>();
            let send_msg_batch_res = self.publish(&self.sqs_update_url, batch_update).await;
            handle_message_batch_result(send_msg_batch_res, keys, &mut failures);
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }
}

fn handle_message_batch_result(
    res: Result<SendMessageBatchOutput, SdkError<SendMessageBatchError, HttpResponse>>,
    target_keys: Vec<ItemKey>,
    failures: &mut Vec<ItemKey>,
) {
    match res {
        Ok(send_msg_batch_res) => {
            for failure in send_msg_batch_res.failed {
                match ItemKey::try_from(failure.id.as_str()) {
                    Ok(key) => {
                        error!(
                            itemKey = %key,
                            errorCode = failure.code,
                            errorMessage = failure.message,
                            "Failed publishing ScrapeItem."
                        );
                        failures.push(key);
                    }
                    Err(err) => {
                        error!(error = %err, payload = ?failure, "Failed converting ItemKey from Batch-ID of partially failed SQS Message-Batch.");
                    }
                }
            }
        }
        Err(err) => {
            error!(error = %err, "Failed publishing ScrapeItems.");
            failures.extend(target_keys);
        }
    }
}

impl<'a> PublishScrapeItemsContext<'a> {
    async fn assess(
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

    async fn publish<T>(
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
}

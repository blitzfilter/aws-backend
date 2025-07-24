use crate::data::ScrapeItem;
use async_stream::try_stream;
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::error::Error;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScraperConfig {
    pub page_delay: Option<Duration>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScrapeError {
    #[error("Encountered Client-Error while scraping.")]
    ClientError(#[from] Box<dyn Error + Send + Sync>),
}

#[async_trait]
pub trait Scraper<Client: Send + Sync>: Send + Sync {
    fn shop_id_str(&self) -> &'static str;
    fn shop_name_str(&self) -> &'static str;

    async fn scrape_page(
        &self,
        client: &Client,
        scraper_config: ScraperConfig,
        page_num: u32,
    ) -> Result<Vec<ScrapeItem>, ScrapeError>;

    fn scrape<'a>(
        &'a self,
        client: &'a Client,
        scraper_config: ScraperConfig,
    ) -> BoxStream<'a, Result<ScrapeItem, ScrapeError>> {
        info!(
            shopId = self.shop_id_str(),
            shopName = self.shop_name_str(),
            config = ?scraper_config,
            "Starting to scrape."
        );
        Box::pin(try_stream! {
            let mut i: u32 = 1;
            loop {
                let items = self.scrape_page(client, scraper_config, i).await?;
                let items_count = items.len();
                info!(shopId = self.shop_id_str(), page = i, total = items_count, "Scraped page.");
                if items_count == 0 {
                    break;
                }
                for item in items {
                    yield item;
                }
                if let Some(duration) = scraper_config.page_delay {
                    tokio::time::sleep(duration).await;
                }
                i += 1;
            }
        })
    }
}

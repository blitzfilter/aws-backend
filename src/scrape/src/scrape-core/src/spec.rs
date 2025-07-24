use crate::data::ScrapeItem;
use async_stream::try_stream;
use async_trait::async_trait;
use common::currency::data::CurrencyData;
use common::language::data::LanguageData;
use futures::stream::BoxStream;
use std::error::Error;
use std::time::Duration;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScraperConfig {
    pub page_delay: Option<Duration>,
    pub language: Option<LanguageData>,
    pub currency: Option<CurrencyData>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScrapeError {
    #[error("Encountered Client-Error while scraping.")]
    ClientError(#[from] Box<dyn Error + Send + Sync>),
}

#[async_trait]
pub trait Scraper: Send + Sync {
    async fn scrape_page(
        &self,
        scraper_config: ScraperConfig,
        page_num: u32,
    ) -> Result<Vec<ScrapeItem>, ScrapeError>;

    fn scrape(&self, scraper_config: ScraperConfig) -> BoxStream<Result<ScrapeItem, ScrapeError>> {
        Box::pin(try_stream! {
            let mut i: u32 = 1;
            loop {
                let items = self.scrape_page(scraper_config, i).await?;
                let items_count = items.len();
                info!(page = i, total = items_count, "Scraped page.");
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

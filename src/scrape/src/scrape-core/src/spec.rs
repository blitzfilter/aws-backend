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

#[cfg(test)]
mod tests {
    use crate::data::ScrapeItem;
    use crate::spec::{ScrapeError, Scraper, ScraperConfig};
    use async_trait::async_trait;
    use futures::StreamExt;
    use item_core::item_state::data::ItemStateData;
    use std::time::{Duration, SystemTime};

    struct DummyClient;
    struct DummyScraper;

    #[async_trait]
    impl Scraper<DummyClient> for DummyScraper {
        fn shop_id_str(&self) -> &'static str {
            "dummy-id"
        }

        fn shop_name_str(&self) -> &'static str {
            "dummy-name"
        }

        async fn scrape_page(
            &self,
            _: &DummyClient,
            _: ScraperConfig,
            page_num: u32,
        ) -> Result<Vec<ScrapeItem>, ScrapeError> {
            let mk_scrape_item = || ScrapeItem {
                shop_id: Default::default(),
                shops_item_id: Default::default(),
                shop_name: "".to_string(),
                title: Default::default(),
                description: Default::default(),
                price: None,
                state: ItemStateData::Listed,
                url: "".to_string(),
                images: vec![],
            };
            match page_num {
                1 => Ok((0..20).map(|_| mk_scrape_item()).collect::<Vec<_>>()),
                2 => Ok((0..20).map(|_| mk_scrape_item()).collect::<Vec<_>>()),
                3 => Ok((0..20).map(|_| mk_scrape_item()).collect::<Vec<_>>()),
                4 => Ok((0..20).map(|_| mk_scrape_item()).collect::<Vec<_>>()),
                5 => Ok((0..1).map(|_| mk_scrape_item()).collect::<Vec<_>>()),
                _ => Ok(vec![]),
            }
        }
    }

    #[tokio::test]
    async fn should_scrape_all_pages_until_no_items_returned() {
        let actual = DummyScraper
            .scrape(&DummyClient, ScraperConfig::default())
            .collect::<Vec<Result<ScrapeItem, ScrapeError>>>()
            .await
            .into_iter()
            .filter_map(Result::ok)
            .collect::<Vec<_>>();

        assert_eq!(81, actual.len());
    }

    #[rstest::rstest]
    #[case(50)]
    #[case(100)]
    #[case(200)]
    #[case(1000)]
    #[case(3000)]
    #[tokio::test]
    async fn should_scrape_all_pages_respecting_configured_inter_page_delay(#[case] delay_ms: u64) {
        let t1 = SystemTime::now();
        let _ = DummyScraper
            .scrape(
                &DummyClient,
                ScraperConfig {
                    page_delay: Some(Duration::from_millis(delay_ms)),
                },
            )
            .collect::<Vec<Result<ScrapeItem, ScrapeError>>>()
            .await
            .into_iter()
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        let t2 = SystemTime::now();

        let time_spent = t2.duration_since(t1).unwrap().as_millis();
        // 5 pages (4 transitions)
        assert!(time_spent > delay_ms as u128 * 4);
    }
}

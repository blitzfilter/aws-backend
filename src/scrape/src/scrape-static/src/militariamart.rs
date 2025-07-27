use async_trait::async_trait;
use common::currency::data::CurrencyData;
use common::language::data::LanguageData;
use common::price::data::PriceData;
use common::shop_id::ShopId;
use common::shops_item_id::ShopsItemId;
use item_core::item_state::data::ItemStateData;
use reqwest::Client;
use scrape_core::data::ScrapeItem;
use scrape_core::spec::{ScrapeError, Scraper, ScraperConfig};
use scraper::{ElementRef, Html, Selector};
use std::collections::HashMap;
use tracing::{info, warn};

#[derive(Debug)]
pub struct MilitariaMart {
    pub id: &'static str,
    pub url: &'static str,
    pub name: &'static str,
    pub shop_dimension: Option<u64>,
    pub language: LanguageData,
}

#[async_trait]
impl Scraper<Client> for MilitariaMart {
    fn shop_id_str(&self) -> &'static str {
        self.id
    }

    fn shop_name_str(&self) -> &'static str {
        self.name
    }

    async fn scrape_page(
        &self,
        client: &Client,
        _: ScraperConfig,
        page_num: u32,
    ) -> Result<Vec<ScrapeItem>, ScrapeError> {
        let html = client
            .get(format!(
                "{}/shop.php?d={}&pg={}",
                &self.url,
                &self.shop_dimension.unwrap_or(1),
                page_num
            ))
            .send()
            .await
            .map_err(Box::from)?
            .text()
            .await
            .map_err(Box::from)?;
        let document = Html::parse_document(&html);
        let shop_items = document
            .select(&Selector::parse("div.shopitem > div.inner-wrapper").unwrap())
            .filter_map(|shop_item| match extract_shops_item_id(shop_item) {
                None => {
                    warn!(
                        shopId = self.shop_id_str(),
                        page = page_num,
                        "Failed extracting ShopsItemId."
                    );
                    None
                }
                Some(shops_item_id) => {
                    let shop_id = self.shop_id_str().into();
                    let item = ScrapeItem {
                        shop_name: self.shop_name_str().into(),
                        title: extract_title(shop_item)
                            .map(|name| HashMap::from([(self.language, name)]))
                            .unwrap_or_default(),
                        description: extract_description(shop_item)
                            .map(|description| HashMap::from([(self.language, description)]))
                            .unwrap_or_default(),
                        price: extract_price(shop_item, &shop_id, &shops_item_id),
                        state: extract_state(shop_item).unwrap_or_else(|| {
                            warn!(
                                shopId = self.shop_id_str(),
                                shopsItemId = shops_item_id.to_string(),
                                page = page_num,
                                "Failed extracting ItemStateData. Defaulting to ItemStateData::Listed."
                            );
                            ItemStateData::Listed
                        }),
                        url: format!("{}/shop.php?code={}", &self.url, shops_item_id),
                        images: extract_relative_image_urls(shop_item)
                            .into_iter()
                            .map(|relative_url| {
                                format!("{}/{}", &self.url, relative_url)
                            })
                            .collect(),
                        shop_id,
                        shops_item_id,
                    };
                    Some(item)
                }
            })
            .collect::<Vec<_>>();

        Ok(shop_items)
    }
}

fn extract_shops_item_id(shop_item: ElementRef) -> Option<ShopsItemId> {
    shop_item
        .select(&Selector::parse("div.block-text > p.itemCode > a").unwrap())
        .next()
        .unwrap()
        .attr("href")
        .and_then(|href| href.strip_prefix("?code="))
        .map(ShopsItemId::from)
}

fn extract_title(shop_item: ElementRef) -> Option<String> {
    shop_item
        .select(&Selector::parse("div.block-text > a.shopitemTitle").unwrap())
        .next()
        .unwrap()
        .attr("title")
        .map(String::from)
}

fn extract_description(shop_item: ElementRef) -> Option<String> {
    // See aws-backend#7
    // This only gathers the description for the catalog-page.
    // It may have been shortened. If so, it ends with '...'.
    // If it does, go to the items page and parse full description there
    shop_item
        .select(&Selector::parse("div.block-text > p.itemDescription").unwrap())
        .next()
        .and_then(|desc_elem| desc_elem.text().next().map(|text| text.trim().to_string()))
}

fn extract_price(
    shop_item: ElementRef,
    shop_id: &ShopId,
    shops_item_id: &ShopsItemId,
) -> Option<PriceData> {
    shop_item
        .select(&Selector::parse("div.block-text > div.actioncontainer > p.price").unwrap())
        .next()
        .and_then(|price_elem| {
            price_elem.text().next().and_then(|price_text| {
                let mut words = price_text.split_whitespace();
                let amount = words
                    .next()
                    .and_then(|price_str| price_str.parse::<f64>().ok());
                let currency = words.next().and_then(|currency_str| match currency_str {
                    "EUR" => Some(CurrencyData::Eur),
                    "GBP" => Some(CurrencyData::Gbp),
                    "USD" => Some(CurrencyData::Usd),
                    "AUD" => Some(CurrencyData::Aud),
                    "CAD" => Some(CurrencyData::Cad),
                    "NZD" => Some(CurrencyData::Nzd),
                    invalid => {
                        warn!(
                            shopId = shop_id.to_string(),
                            shopsItemId = shops_item_id.to_string(),
                            payload = invalid,
                            "Found invalid CurrencyData."
                        );
                        None
                    }
                });
                if let (Some(amount), Some(currency)) = (amount, currency) {
                    match PriceData::new_f64(currency, amount) {
                        Ok(price_data) => Some(price_data),
                        Err(err) => {
                            info!(
                                error = %err,
                                shopId = shop_id.to_string(),
                                shopsItemId = shops_item_id.to_string(),
                                payload = amount,
                                "Found negative monetary amount."
                            );
                            None
                        }
                    }
                } else {
                    None
                }
            })
        })
}

fn extract_state(shop_item: ElementRef) -> Option<ItemStateData> {
    let selectors = [
        "div.block-text > div.actioncontainer > form > button",
        "div.block-text > div.actioncontainer > form > p",
    ];

    selectors
        .iter()
        .filter_map(|selector_str| {
            let selector = Selector::parse(selector_str).ok()?;
            shop_item.select(&selector).next()
        })
        .find_map(|state_elem| {
            state_elem.text().next().map(|state_text| match state_text {
                "SOLD" => ItemStateData::Sold,
                "Reserved" => ItemStateData::Reserved,
                "Add to basket" => ItemStateData::Available,
                _ => ItemStateData::Listed,
            })
        })
        .or(Some(ItemStateData::Listed))
}

fn extract_relative_image_urls(shop_item: ElementRef) -> Vec<String> {
    shop_item
        .select(&Selector::parse("div.block-image > a > img").unwrap())
        .next()
        .unwrap()
        .attr("src")
        .map(String::from)
        .map(|relative_url| vec![relative_url])
        .unwrap_or_default()
}

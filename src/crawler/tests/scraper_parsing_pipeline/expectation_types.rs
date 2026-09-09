use money::Price;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_price::ProductListingPrice,
};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RawExpectation {
    pub source_listing_id: String,
    pub title: String,
    pub description: Vec<String>,
    pub price: Option<String>,
    pub price_estimate_min: Option<String>,
    pub price_estimate_max: Option<String>,
    pub state: String,
    pub images: Vec<String>,
    pub auction_start: Option<String>,
    pub auction_end: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NormalizedExpectation {
    pub source_listing_id: String,
    pub title: String,
    pub description: Option<String>,
    pub price: Option<ProductListingPrice>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub availability: Option<ListingAvailability>,
    pub url: String,
    pub images: Vec<String>,
    pub auction_start: Option<time::OffsetDateTime>,
    pub auction_end: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NormalizedExpectationJson {
    pub source_listing_id: String,
    pub title: String,
    pub description: Option<String>,
    pub price: Option<u64>,
    pub price_currency: Option<String>,
    pub price_estimate_min: Option<u64>,
    pub price_estimate_min_currency: Option<String>,
    pub price_estimate_max: Option<u64>,
    pub price_estimate_max_currency: Option<String>,
    pub availability: Option<String>,
    pub url: String,
    pub images: Vec<String>,
    pub auction_start: Option<String>,
    pub auction_end: Option<String>,
}

use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;
use product_listing_core::{
    listing_availability::ListingAvailability,
    product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
    product_listing_id::ProductListingId,
    source_listing_id::SourceListingId,
};
use product_listing_service::use_cases::{
    ProductListingDiscoveryHistory, ProductListingHistoryChange, ProductListingHistoryEntry,
    ProductListingHistoryEntryKind,
};
use serde::Serialize;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingHistoryEntryData {
    event_type: &'static str,
    product_listing_id: ProductListingId,
    event_id: EventId,
    payload: ProductListingHistoryPayloadData,
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ProductListingHistoryPayloadData {
    Discovered(Box<ProductListingDiscoveryHistoryData>),
    Changed(ProductListingChangedHistoryData),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingDiscoveryHistoryData {
    listing_source_id: ListingSourceId,
    #[serde(serialize_with = "crate::wire::source_listing_id::serialize")]
    source_listing_id: SourceListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<LocalizedTextData>,
    pricing: ProductListingPricingData,
    #[serde(with = "crate::wire::listing_availability::option")]
    availability: Option<ListingAvailability>,
    url: Url,
    image_count: u64,
    auction: ProductListingAuctionData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingChangedHistoryData {
    changes: Vec<ProductListingHistoryChangeData>,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
enum ProductListingHistoryChangeData {
    MainPriceChanged {
        previous: Option<ProductListingPriceData>,
        current: Option<ProductListingPriceData>,
    },
    MinimumEstimateChanged {
        previous: Option<PriceData>,
        current: Option<PriceData>,
    },
    MaximumEstimateChanged {
        previous: Option<PriceData>,
        current: Option<PriceData>,
    },
    AvailabilityChanged {
        #[serde(with = "crate::wire::listing_availability::option")]
        previous: Option<ListingAvailability>,
        #[serde(with = "crate::wire::listing_availability::option")]
        current: Option<ListingAvailability>,
    },
    UrlChanged {
        previous: Url,
        current: Url,
    },
    ImagesChanged {
        previous_count: u64,
        current_count: u64,
    },
    AuctionChanged {
        previous: ProductListingAuctionData,
        current: ProductListingAuctionData,
    },
    Withdrawn {
        #[serde(with = "crate::wire::listing_availability::option")]
        previous_availability: Option<ListingAvailability>,
    },
    Restored,
    SaleObserved {
        observation: ListingSaleObservationHistoryData,
    },
    SaleObservationRetracted {
        observation: ListingSaleObservationHistoryData,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<ProductListingPriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<PriceData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListingSaleObservationHistoryData {
    #[serde(with = "time::serde::rfc3339")]
    observed_at: OffsetDateTime,
    fx_rate_id: FxRateId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingAuctionData {
    #[serde(with = "time::serde::rfc3339::option")]
    start: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    end: Option<OffsetDateTime>,
}

impl From<ProductListingHistoryEntry> for ProductListingHistoryEntryData {
    fn from(entry: ProductListingHistoryEntry) -> Self {
        Self {
            event_type: entry.event_type().as_str(),
            product_listing_id: entry.product_listing_id,
            event_id: entry.event_id,
            payload: entry.kind.into(),
            timestamp: entry.occurred_at,
        }
    }
}

impl From<ProductListingHistoryEntryKind> for ProductListingHistoryPayloadData {
    fn from(value: ProductListingHistoryEntryKind) -> Self {
        match value {
            ProductListingHistoryEntryKind::Discovered(discovery) => {
                Self::Discovered(Box::new((*discovery).into()))
            }
            ProductListingHistoryEntryKind::Changed(changes) => {
                Self::Changed(ProductListingChangedHistoryData {
                    changes: changes.into_inner().into_iter().map(Into::into).collect(),
                })
            }
        }
    }
}

impl From<ProductListingDiscoveryHistory> for ProductListingDiscoveryHistoryData {
    fn from(value: ProductListingDiscoveryHistory) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            source_listing_id: value.source_listing_id,
            title: value.title.map(Into::into),
            description: value.description.map(Into::into),
            pricing: value.pricing.into(),
            availability: value.availability,
            url: value.url,
            image_count: value.image_count,
            auction: value.auction.into(),
        }
    }
}

impl From<ProductListingHistoryChange> for ProductListingHistoryChangeData {
    fn from(value: ProductListingHistoryChange) -> Self {
        match value {
            ProductListingHistoryChange::MainPriceChanged { previous, current } => {
                Self::MainPriceChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::MinimumEstimateChanged { previous, current } => {
                Self::MinimumEstimateChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::MaximumEstimateChanged { previous, current } => {
                Self::MaximumEstimateChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::AvailabilityChanged { previous, current } => {
                Self::AvailabilityChanged { previous, current }
            }
            ProductListingHistoryChange::UrlChanged { previous, current } => {
                Self::UrlChanged { previous, current }
            }
            ProductListingHistoryChange::ImagesChanged {
                previous_count,
                current_count,
            } => Self::ImagesChanged {
                previous_count,
                current_count,
            },
            ProductListingHistoryChange::AuctionChanged { previous, current } => {
                Self::AuctionChanged {
                    previous: previous.into(),
                    current: current.into(),
                }
            }
            ProductListingHistoryChange::Withdrawn {
                previous_availability,
            } => Self::Withdrawn {
                previous_availability,
            },
            ProductListingHistoryChange::Restored => Self::Restored,
            ProductListingHistoryChange::SaleObserved { observation } => Self::SaleObserved {
                observation: observation.into(),
            },
            ProductListingHistoryChange::SaleObservationRetracted { observation } => {
                Self::SaleObservationRetracted {
                    observation: observation.into(),
                }
            }
        }
    }
}

impl From<ProductListingPricing> for ProductListingPricingData {
    fn from(pricing: ProductListingPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<ListingSaleObservation> for ListingSaleObservationHistoryData {
    fn from(observation: ListingSaleObservation) -> Self {
        Self {
            observed_at: observation.observed_at(),
            fx_rate_id: observation.fx_rate_id(),
        }
    }
}

impl From<ProductListingAuction> for ProductListingAuctionData {
    fn from(auction: ProductListingAuction) -> Self {
        Self {
            start: auction.start,
            end: auction.end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_listing_core::listing_availability::ListingAvailability;
    use product_listing_service::use_cases::ProductListingHistoryChanges;
    use serde_json::json;

    #[test]
    fn should_serialize_one_changed_history_entry_with_ordered_changes() {
        let entry = ProductListingHistoryEntry {
            product_listing_id: ProductListingId::new(),
            event_id: EventId::new(),
            occurred_at: OffsetDateTime::now_utc(),
            kind: ProductListingHistoryEntryKind::Changed(
                ProductListingHistoryChanges::try_from(vec![
                    ProductListingHistoryChange::AvailabilityChanged {
                        previous: Some(ListingAvailability::Available),
                        current: Some(ListingAvailability::SoldOut),
                    },
                    ProductListingHistoryChange::ImagesChanged {
                        previous_count: 2,
                        current_count: 2,
                    },
                    ProductListingHistoryChange::Restored,
                ])
                .unwrap_or_else(|error| panic!("non-empty history changes: {error}")),
            ),
        };

        let data = ProductListingHistoryEntryData::from(entry);
        let value = serde_json::to_value(data)
            .unwrap_or_else(|error| panic!("serialize ProductListing history entry: {error}"));

        assert_eq!(json!("PRODUCT_LISTING_CHANGED"), value["eventType"]);
        assert_eq!(
            json!([
                {
                    "type": "AVAILABILITY_CHANGED",
                    "previous": "AVAILABLE",
                    "current": "SOLD_OUT"
                },
                {
                    "type": "IMAGES_CHANGED",
                    "previousCount": 2,
                    "currentCount": 2
                },
                {
                    "type": "RESTORED"
                }
            ]),
            value["payload"]["changes"]
        );
    }
}

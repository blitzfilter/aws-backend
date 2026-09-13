use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use application::operation_context::Principal;
use auction_core::{AuctionFormat, AuctionId, AuctionReportedStatus, AuctionSchedule, AuctionTime};
use auction_service::ports::AuctionSummary;
use axum::Json;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use domain_primitives::event_id::EventId;

use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;

use notification_core::{
    notification_id::NotificationId, presentation::NotificationImagePresentation,
};
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing::{
    LotAuctionTiming, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use product_listing_core::content_policy::ContentPolicyDecision;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_service::ports::ListingSourceSummary;
use product_listing_service::use_cases::{
    DisplayProductListingPricing, PersonalizedProductListingDetailsView,
    PersonalizedProductListingSummary, ProductListingDetailsView,
    ProductListingPricingPresentation, ProductListingPricingValuation, ProductListingSummary,
    ProductListingSummaryPriceValuation,
};
use product_listing_service::user_state::{
    ContentVisibilityUserState, NotificationUserState, ProductListingUserState,
    SearchFilterUserState, WatchlistUserState,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersonalizedData<ItemData, UserStateData> {
    pub(crate) item: ItemData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user_state: Option<UserStateData>,
}

use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingDetailsData {
    product_listing_id: ProductListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_listing_title_slug_id: Option<ProductListingSlugId>,
    event_id: EventId,
    source: ListingSourceSummaryData,
    #[serde(serialize_with = "crate::wire::source_listing_id::serialize")]
    source_listing_id: SourceListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_description: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<LocalizedTextData>,
    pricing: ProductListingPricingPresentationData,
    #[serde(with = "crate::wire::listing_availability::option")]
    availability: Option<ListingAvailability>,
    #[serde(with = "crate::wire::listing_lifecycle")]
    lifecycle: ListingLifecycle,
    url: Url,
    view_url: Url,
    images: Vec<ProductListingImageData>,
    content_policy: Option<ContentPolicyData>,
    auction: Option<ProductListingAuctionData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auction_summary: Option<AuctionSummaryData>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingUserStateData {
    watchlist: WatchlistUserStateData,
    content_visibility: ContentVisibilityUserStateData,
    notification: NotificationUserStateData,
    search_filter: SearchFilterUserStateData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchlistUserStateData {
    watching: bool,
    notifications: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ContentVisibilityUserStateData {
    show_unassessed_or_sensitive_content: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotificationUserStateData {
    unseen_notification_ids: Vec<NotificationId>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchFilterUserStateData {
    matched: bool,
    hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_search_filter_id: Option<UserSearchFilterId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_search_filter_name: Option<UserSearchFilterName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    match_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    match_feedback: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListingSourceSummaryData {
    listing_source_id: ListingSourceId,
    name: String,
    slug_id: String,
}

impl From<ListingSourceSummary> for ListingSourceSummaryData {
    fn from(source: ListingSourceSummary) -> Self {
        Self {
            listing_source_id: source.listing_source_id,
            name: source.name.as_ref().to_owned(),
            slug_id: source.slug_id.to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingSummaryData {
    product_listing_id: ProductListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_listing_title_slug_id: Option<ProductListingSlugId>,
    event_id: EventId,
    source: ListingSourceSummaryData,
    #[serde(serialize_with = "crate::wire::source_listing_id::serialize")]
    source_listing_id: SourceListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    auction_id: Option<AuctionId>,
    has_auction_context: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    auction_summary: Option<AuctionSummaryData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_price: Option<ProductListingPriceData>,
    price_valuation: ProductListingSummaryPriceValuationData,
    #[serde(with = "crate::wire::listing_availability::option")]
    availability: Option<ListingAvailability>,
    #[serde(with = "crate::wire::listing_lifecycle")]
    lifecycle: ListingLifecycle,
    url: Url,
    view_url: Url,
    images: Vec<ProductListingImageData>,
    content_policy: Option<ContentPolicyData>,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingPresentationData {
    source: ProductListingPricingData,
    display: ProductListingPricingData,
    valuation: ProductListingPricingValuationData,
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
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductListingPricingValuationData {
    Current {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
    },
    SaleObservation {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
        #[serde(rename = "observedAt", with = "time::serde::rfc3339")]
        observed_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum ProductListingSummaryPriceValuationData {
    Current {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "capturedAt", with = "time::serde::rfc3339")]
        captured_at: OffsetDateTime,
    },
    SaleObservation {
        #[serde(rename = "fxRateId")]
        fx_rate_id: FxRateId,
        #[serde(rename = "observedAt", with = "time::serde::rfc3339")]
        observed_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingImageData {
    url: Option<Url>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "decision", rename_all = "SCREAMING_SNAKE_CASE")]
enum ContentPolicyData {
    Allowed,
    RequiresConsent { category: &'static str },
}

impl From<ContentPolicyDecision> for ContentPolicyData {
    fn from(value: ContentPolicyDecision) -> Self {
        match value {
            ContentPolicyDecision::Allowed => Self::Allowed,
            ContentPolicyDecision::RequiresConsent(category) => Self::RequiresConsent {
                category: category.as_str(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductListingAuctionData {
    #[serde(skip_serializing_if = "Option::is_none")]
    auction_id: Option<AuctionId>,
    lot_number: Option<String>,
    catalogue_position: Option<u32>,
    timing: Option<LotAuctionTimingData>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LotAuctionTimingData {
    bidding_opens: Option<AuctionTimeData>,
    scheduled_closes: Option<AuctionTimeData>,
    #[serde(with = "time::serde::rfc3339::option")]
    reported_closed_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "precision", rename_all = "SCREAMING_SNAKE_CASE")]
enum AuctionTimeData {
    Instant {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        #[serde(rename = "sourceTimezone", skip_serializing_if = "Option::is_none")]
        source_timezone: Option<String>,
    },
    Date {
        on: String,
        #[serde(rename = "sourceTimezone", skip_serializing_if = "Option::is_none")]
        source_timezone: Option<String>,
    },
}

impl ProductListingDetailsData {
    fn from_view(view: ProductListingDetailsView) -> Self {
        Self {
            product_listing_id: view.product_listing_id,
            product_listing_title_slug_id: view.product_listing_title_slug_id,
            event_id: view.event_id,
            source: view.source.into(),
            source_listing_id: view.source_listing_id,
            product_title: view.product_title.map(Into::into),
            product_description: view.product_description.map(Into::into),
            title: view.title.map(Into::into),
            description: view.description.map(Into::into),
            pricing: view.pricing.into(),
            availability: view.availability,
            lifecycle: view.lifecycle,
            url: view.url,
            view_url: view.view_url,
            images: view.images.into_iter().map(Into::into).collect(),
            content_policy: view.content_policy.map(Into::into),
            auction: view.auction.map(Into::into),
            auction_summary: view.auction_summary.map(Into::into),
            created: view.created,
            updated: view.updated,
        }
    }
}

impl From<ProductListingDetailsView> for ProductListingDetailsData {
    fn from(view: ProductListingDetailsView) -> Self {
        Self::from_view(view)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuctionSummaryData {
    auction_id: AuctionId,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reported_status: Option<&'static str>,
    schedule: AuctionScheduleData,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuctionScheduleData {
    bidding_opens: Option<AuctionTimeData>,
    live_starts: Option<AuctionTimeData>,
    lots_begin_closing: Option<AuctionTimeData>,
    scheduled_end: Option<AuctionTimeData>,
}

impl From<AuctionSummary> for AuctionSummaryData {
    fn from(value: AuctionSummary) -> Self {
        Self {
            auction_id: value.auction_id,
            name: value.name.map(|name| LocalizedTextData {
                text: name.payload.as_ref().to_owned(),
                language: name.localization,
            }),
            format: value.format.map(AuctionFormat::as_str),
            reported_status: value.reported_status.map(AuctionReportedStatus::as_str),
            schedule: value.schedule.into(),
        }
    }
}

impl From<AuctionSchedule> for AuctionScheduleData {
    fn from(value: AuctionSchedule) -> Self {
        Self {
            bidding_opens: value.bidding_opens().cloned().map(Into::into),
            live_starts: value.live_starts().cloned().map(Into::into),
            lots_begin_closing: value.lots_begin_closing().cloned().map(Into::into),
            scheduled_end: value.scheduled_end().cloned().map(Into::into),
        }
    }
}

impl From<ProductListingAuction> for ProductListingAuctionData {
    fn from(auction: ProductListingAuction) -> Self {
        Self {
            auction_id: auction
                .membership()
                .map(|membership| membership.auction_id()),
            lot_number: auction.lot_number().map(ToString::to_string),
            catalogue_position: auction
                .catalogue_position()
                .map(|position| position.value()),
            timing: auction.timing().cloned().map(Into::into),
        }
    }
}

impl From<LotAuctionTiming> for LotAuctionTimingData {
    fn from(timing: LotAuctionTiming) -> Self {
        Self {
            bidding_opens: timing.bidding_opens().cloned().map(Into::into),
            scheduled_closes: timing.scheduled_closes().cloned().map(Into::into),
            reported_closed_at: timing.reported_closed_at(),
        }
    }
}

impl From<AuctionTime> for AuctionTimeData {
    fn from(value: AuctionTime) -> Self {
        match value {
            AuctionTime::Instant {
                at,
                source_timezone,
            } => Self::Instant {
                at,
                source_timezone: source_timezone.map(String::from),
            },
            AuctionTime::Date {
                on,
                source_timezone,
            } => Self::Date {
                on: on.to_string(),
                source_timezone: source_timezone.map(String::from),
            },
        }
    }
}

impl From<ProductListingPricingPresentation> for ProductListingPricingPresentationData {
    fn from(pricing: ProductListingPricingPresentation) -> Self {
        Self {
            source: pricing.source.into(),
            display: pricing.display.into(),
            valuation: pricing.valuation.into(),
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

impl From<DisplayProductListingPricing> for ProductListingPricingData {
    fn from(pricing: DisplayProductListingPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<ProductListingSummaryPriceValuation> for ProductListingSummaryPriceValuationData {
    fn from(valuation: ProductListingSummaryPriceValuation) -> Self {
        match valuation {
            ProductListingSummaryPriceValuation::Current {
                fx_rate_id,
                captured_at,
            } => Self::Current {
                fx_rate_id,
                captured_at,
            },
            ProductListingSummaryPriceValuation::SaleObservation {
                fx_rate_id,
                observed_at,
            } => Self::SaleObservation {
                fx_rate_id,
                observed_at,
            },
        }
    }
}

impl From<ProductListingPricingValuation> for ProductListingPricingValuationData {
    fn from(valuation: ProductListingPricingValuation) -> Self {
        match valuation {
            ProductListingPricingValuation::Current {
                fx_rate_id,
                captured_at,
            } => Self::Current {
                fx_rate_id,
                captured_at,
            },
            ProductListingPricingValuation::SaleObservation {
                fx_rate_id,
                captured_at,
                observed_at,
            } => Self::SaleObservation {
                fx_rate_id,
                captured_at,
                observed_at,
            },
        }
    }
}

impl From<ProductListingUserState> for ProductListingUserStateData {
    fn from(state: ProductListingUserState) -> Self {
        Self {
            watchlist: state.watchlist.into(),
            content_visibility: state.content_visibility.into(),
            notification: state.notification.into(),
            search_filter: state.search_filter.into(),
        }
    }
}

impl From<WatchlistUserState> for WatchlistUserStateData {
    fn from(state: WatchlistUserState) -> Self {
        Self {
            watching: state.watching,
            notifications: state.notifications,
        }
    }
}

impl From<ContentVisibilityUserState> for ContentVisibilityUserStateData {
    fn from(state: ContentVisibilityUserState) -> Self {
        Self {
            show_unassessed_or_sensitive_content: state.show_unassessed_or_sensitive_content,
        }
    }
}

impl From<NotificationUserState> for NotificationUserStateData {
    fn from(state: NotificationUserState) -> Self {
        Self {
            unseen_notification_ids: state.unseen_notification_ids,
        }
    }
}

impl From<SearchFilterUserState> for SearchFilterUserStateData {
    fn from(state: SearchFilterUserState) -> Self {
        Self {
            matched: state.matched,
            hidden: state.hidden,
            user_search_filter_id: state.user_search_filter_id,
            user_search_filter_name: state.user_search_filter_name,
            match_reason: state.match_reason.map(|reason| reason.to_string()),
            match_feedback: state.match_feedback,
        }
    }
}

impl ProductListingSummaryData {
    fn from_view(summary: ProductListingSummary) -> Self {
        Self {
            product_listing_id: summary.product_listing_id,
            product_listing_title_slug_id: summary.product_listing_title_slug_id,
            event_id: summary.event_id,
            source: summary.source.into(),
            source_listing_id: summary.source_listing_id,
            auction_id: summary.auction_id,
            has_auction_context: summary.has_auction_context,
            auction_summary: summary.auction_summary.map(Into::into),
            title: summary.title.map(Into::into),
            display_price: summary.display_price.map(Into::into),
            price_valuation: summary.price_valuation.into(),
            availability: summary.availability,
            lifecycle: summary.lifecycle,
            url: summary.url,
            view_url: summary.view_url,
            images: summary.images.into_iter().map(Into::into).collect(),
            content_policy: summary.content_policy.map(Into::into),
            updated: summary.updated,
        }
    }
}

impl From<ProductListingSummary> for ProductListingSummaryData {
    fn from(summary: ProductListingSummary) -> Self {
        Self::from_view(summary)
    }
}

impl ProductListingImageData {
    pub(crate) fn from_presented(image: NotificationImagePresentation) -> Self {
        Self { url: image.url }
    }
}

impl From<product_listing_service::use_cases::ProductListingImageView> for ProductListingImageData {
    fn from(image: product_listing_service::use_cases::ProductListingImageView) -> Self {
        Self { url: image.url }
    }
}

pub(crate) type PersonalizedProductListingDetailsData =
    PersonalizedData<ProductListingDetailsData, ProductListingUserStateData>;
pub(crate) type PersonalizedProductListingSummaryData =
    PersonalizedData<ProductListingSummaryData, ProductListingUserStateData>;

pub(crate) fn personalized_product_details_data(
    personalized: PersonalizedProductListingDetailsView,
) -> PersonalizedProductListingDetailsData {
    PersonalizedData {
        item: ProductListingDetailsData::from_view(personalized.item),
        user_state: personalized.user_state.map(Into::into),
    }
}

pub(crate) fn personalized_product_summary_data(
    personalized: PersonalizedProductListingSummary,
) -> PersonalizedProductListingSummaryData {
    PersonalizedData {
        item: ProductListingSummaryData::from_view(personalized.item),
        user_state: personalized.user_state.map(Into::into),
    }
}

pub(crate) fn product_response(
    view: PersonalizedProductListingDetailsView,
    principal: &Principal,
) -> Response {
    let lifecycle = view.item.lifecycle;
    let content_language = view
        .item
        .title
        .as_ref()
        .map(|title| title.localization.as_str());
    let mut response = Json(personalized_product_details_data(view)).into_response();
    let cache_control = match principal {
        Principal::Anonymous if matches!(lifecycle, ListingLifecycle::Withdrawn) => {
            "public, max-age=180, s-maxage=86400"
        }
        Principal::Anonymous => "public, max-age=180, s-maxage=900",
        Principal::User(_)
        | Principal::DelegatedUser { .. }
        | Principal::Service(_)
        | Principal::System => "no-store",
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_control),
    );
    if let Some(language) = content_language {
        headers.insert(header::CONTENT_LANGUAGE, HeaderValue::from_static(language));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::{AuctionName, AuctionTime};
    use localization::Localized;
    use time::macros::{date, datetime};

    #[test]
    fn should_serialize_only_safe_auction_summary_fields_with_time_precision() {
        let auction_id = AuctionId::new();
        let summary = AuctionSummary {
            auction_id,
            name: Some(Localized::new(
                localization::Language::En,
                AuctionName::try_from("Autumn Decorative Arts")
                    .unwrap_or_else(|error| panic!("valid test auction name: {error}")),
            )),
            format: Some(AuctionFormat::Timed),
            reported_status: Some(AuctionReportedStatus::Scheduled),
            schedule: AuctionSchedule::new(
                None,
                Some(AuctionTime::instant(datetime!(2026-10-18 16:00 UTC), None)),
                None,
                Some(AuctionTime::date(date!(2026 - 10 - 19), None)),
            )
            .unwrap_or_else(|error| panic!("valid test auction schedule: {error}")),
        };

        let value = serde_json::to_value(AuctionSummaryData::from(summary))
            .unwrap_or_else(|error| panic!("serialize auction summary: {error}"));

        assert_eq!(
            Some(&serde_json::Value::String(auction_id.to_string())),
            value.get("auctionId")
        );
        assert_eq!(
            Some(&serde_json::Value::String("TIMED".to_owned())),
            value.get("format")
        );
        assert_eq!(
            Some(&serde_json::Value::String("SCHEDULED".to_owned())),
            value.get("reportedStatus")
        );
        assert_eq!(
            Some(&serde_json::Value::String("INSTANT".to_owned())),
            value.pointer("/schedule/liveStarts/precision")
        );
        assert_eq!(
            Some(&serde_json::Value::String("DATE".to_owned())),
            value.pointer("/schedule/scheduledEnd/precision")
        );
        assert!(value.get("sourceAuctionId").is_none());
        assert!(value.get("version").is_none());
    }

    #[test]
    fn should_serialize_a_presented_redacted_image() {
        let data = ProductListingImageData::from(
            product_listing_service::use_cases::ProductListingImageView { url: None },
        );
        assert_eq!(data.url, None);
    }
}

use crate::error::{ApiError, BAD_BODY_VALUE};
use crate::product_listings::product_data::ProductListingImageData;
use crate::values::{LocalizedTextData, ProductListingPriceData};
use axum::response::{IntoResponse, Response};
use listing_source_core::listing_source_id::ListingSourceId;
use localization::{Language, Localized};

use notification_core::{
    notification::{
        LocalizedNotificationContent, LocalizedNotificationWatchlistChange,
        LocalizedProductListingNotificationSnapshot, PartnershipApplicationDecision,
    },
    notification_id::NotificationId,
    notification_kind::NotificationKind,
    presentation::present_image,
};
use notification_service::presentation::NotificationPresentationPreferences;
use notification_service::use_cases::queries::list_notifications::ListedNotification;
use partnership_core::partnership_application_id::PartnershipApplicationId;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_id::ProductListingId,
    product_listing_price::ProductListingPrice, title::Title,
};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use serde::{Deserialize, Serialize, Serializer};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateNotificationSeenData {
    pub seen: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateNotificationsSeenData {
    pub notification_ids: Vec<String>,
    pub seen: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NotificationData {
    notification_id: NotificationId,
    seen: bool,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    #[serde(with = "crate::wire::notification_kind")]
    kind: NotificationKind,
    payload: NotificationContentData,
}

impl From<(ListedNotification, NotificationPresentationPreferences)> for NotificationData {
    fn from(
        (value, presentation_preferences): (
            ListedNotification,
            NotificationPresentationPreferences,
        ),
    ) -> Self {
        Self {
            notification_id: value.notification_id,
            seen: value.seen,
            created: value.created,
            updated: value.updated,
            kind: value.kind,
            payload: (value.content, presentation_preferences).into(),
        }
    }
}

#[derive(Debug)]
enum NotificationContentData {
    Watchlist(WatchlistNotificationPayloadData),
    SearchFilter(SearchFilterNotificationPayloadData),
    PartnershipApplication(PartnershipApplicationNotificationPayloadData),
}

impl Serialize for NotificationContentData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Watchlist(payload) => payload.serialize(serializer),
            Self::SearchFilter(payload) => payload.serialize(serializer),
            Self::PartnershipApplication(payload) => payload.serialize(serializer),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchlistNotificationPayloadData {
    product_listing_id: ProductListingId,
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    listing_source_slug_id: String,
    product_listing_title_slug_id: String,
    listing_source_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductListingImageData>,
    url: Url,
    view_url: Url,
    change: WatchlistNotificationChangeData,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
enum WatchlistNotificationChangeData {
    PriceChange {
        old_price: Option<ProductListingPriceData>,
        new_price: Option<ProductListingPriceData>,
    },
    AvailabilityChange {
        #[serde(with = "crate::wire::listing_availability::option")]
        old_availability: Option<ListingAvailability>,
        #[serde(with = "crate::wire::listing_availability::option")]
        new_availability: Option<ListingAvailability>,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchFilterNotificationPayloadData {
    product_listing_id: ProductListingId,
    user_search_filter_id: UserSearchFilterId,
    user_search_filter_name: String,
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    listing_source_slug_id: String,
    product_listing_title_slug_id: String,
    listing_source_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductListingImageData>,
    url: Url,
    view_url: Url,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PartnershipApplicationNotificationPayloadData {
    partnership_application_id: PartnershipApplicationId,
    #[serde(with = "crate::wire::partnership_application_decision")]
    decision: PartnershipApplicationDecision,
    party_name: String,
    listing_source_name: String,
    image: Option<Url>,
}

impl
    From<(
        LocalizedNotificationContent,
        NotificationPresentationPreferences,
    )> for NotificationContentData
{
    fn from(
        (value, presentation_preferences): (
            LocalizedNotificationContent,
            NotificationPresentationPreferences,
        ),
    ) -> Self {
        match value {
            LocalizedNotificationContent::Watchlist {
                product_listing_id,
                snapshot,
                change,
            } => {
                let snapshot = notification_product_snapshot(snapshot, presentation_preferences);
                Self::Watchlist(WatchlistNotificationPayloadData {
                    product_listing_id,
                    listing_source_id: snapshot.listing_source_id,
                    source_listing_id: snapshot.source_listing_id,
                    listing_source_slug_id: snapshot.listing_source_slug_id,
                    product_listing_title_slug_id: snapshot.product_listing_title_slug_id,
                    listing_source_name: snapshot.listing_source_name,
                    title: snapshot.title,
                    image: snapshot.image,
                    url: snapshot.url,
                    view_url: snapshot.view_url,
                    change: change.into(),
                })
            }
            LocalizedNotificationContent::SearchFilter {
                product_listing_id,
                snapshot,
                user_search_filter_id,
                user_search_filter_name,
            } => {
                let snapshot = notification_product_snapshot(snapshot, presentation_preferences);
                Self::SearchFilter(SearchFilterNotificationPayloadData {
                    product_listing_id,
                    user_search_filter_id,
                    user_search_filter_name: user_search_filter_name.to_string(),
                    listing_source_id: snapshot.listing_source_id,
                    source_listing_id: snapshot.source_listing_id.to_string(),
                    listing_source_slug_id: snapshot.listing_source_slug_id.to_string(),
                    product_listing_title_slug_id: snapshot
                        .product_listing_title_slug_id
                        .to_string(),
                    listing_source_name: snapshot.listing_source_name.to_string(),
                    title: snapshot.title,
                    image: snapshot.image,
                    url: snapshot.url,
                    view_url: snapshot.view_url,
                })
            }

            LocalizedNotificationContent::PartnershipApplication {
                partnership_application_id,
                snapshot,
                decision,
            } => Self::PartnershipApplication(PartnershipApplicationNotificationPayloadData {
                partnership_application_id,
                decision,
                party_name: snapshot.party_name.to_string(),
                listing_source_name: snapshot.listing_source_name.to_string(),
                image: snapshot.image,
            }),
        }
    }
}

fn notification_product_snapshot(
    snapshot: LocalizedProductListingNotificationSnapshot,
    presentation_preferences: NotificationPresentationPreferences,
) -> NotificationProductListingSnapshotData {
    NotificationProductListingSnapshotData {
        listing_source_id: snapshot.listing_source_id,
        source_listing_id: snapshot.source_listing_id.to_string(),
        listing_source_slug_id: snapshot.listing_source_slug_id.to_string(),
        product_listing_title_slug_id: snapshot.product_listing_title_slug_id.to_string(),
        listing_source_name: snapshot.listing_source_name.to_string(),
        title: snapshot.title.map(localized_text_data),
        image: present_image(
            snapshot.image,
            snapshot.content_policy,
            presentation_preferences.show_unassessed_or_sensitive_content,
        )
        .map(ProductListingImageData::from_presented),
        url: snapshot.url,
        view_url: snapshot.view_url,
    }
}

fn localized_text_data(value: Localized<Language, Title>) -> LocalizedTextData {
    LocalizedTextData {
        text: value.payload.into(),
        language: value.localization,
    }
}

fn product_listing_price_data(value: ProductListingPrice) -> ProductListingPriceData {
    value.into()
}

struct NotificationProductListingSnapshotData {
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    listing_source_slug_id: String,
    product_listing_title_slug_id: String,
    listing_source_name: String,
    title: Option<LocalizedTextData>,
    image: Option<ProductListingImageData>,
    url: Url,
    view_url: Url,
}

impl From<LocalizedNotificationWatchlistChange> for WatchlistNotificationChangeData {
    fn from(value: LocalizedNotificationWatchlistChange) -> Self {
        match value {
            LocalizedNotificationWatchlistChange::PriceChange {
                old_price,
                new_price,
            } => Self::PriceChange {
                old_price: old_price.map(product_listing_price_data),
                new_price: new_price.map(product_listing_price_data),
            },
            LocalizedNotificationWatchlistChange::AvailabilityChange {
                old_availability,
                new_availability,
            } => Self::AvailabilityChange {
                old_availability,
                new_availability,
            },
        }
    }
}

#[allow(clippy::result_large_err)]
pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T, Response> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("Request body is required.")
            .into_response());
    }
    serde_json::from_str(body).map_err(|error| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail(error.to_string())
            .into_response()
    })
}

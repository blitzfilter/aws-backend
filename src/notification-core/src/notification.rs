use crate::{notification_id::NotificationId, notification_kind::NotificationKind};
use domain_primitives::event_id::EventId;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use localization::Localized;

use partnership_core::partnership_application_id::PartnershipApplicationId;
use party_core::party_name::PartyName;
use product_listing_core::product_listing_price::ProductListingPrice;
use product_listing_core::{
    content_policy::ContentPolicyDecision, listing_availability::ListingAvailability,
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId, title::Title,
};
use search_filter_core::{
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use std::collections::HashMap;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    notification_id: NotificationId,
    user_id: UserId,
    content: NotificationContent,
    seen: bool,
}

impl Notification {
    pub fn new(
        notification_id: NotificationId,
        user_id: UserId,
        content: NotificationContent,
    ) -> Self {
        Self {
            notification_id,
            user_id,
            content,
            seen: false,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(
        state: RehydratedNotificationState,
    ) -> Result<Self, RehydrateNotificationError> {
        Ok(Self {
            notification_id: state.notification_id,
            user_id: state.user_id,
            content: state.content,
            seen: state.seen,
        })
    }

    pub fn notification_id(&self) -> NotificationId {
        self.notification_id
    }
    pub fn user_id(&self) -> UserId {
        self.user_id
    }
    pub fn content(&self) -> &NotificationContent {
        &self.content
    }
    pub fn kind(&self) -> NotificationKind {
        self.content.kind()
    }
    pub fn seen(&self) -> bool {
        self.seen
    }
    pub fn origin_event_id(&self) -> Option<EventId> {
        self.content.origin_event_id()
    }
    pub fn product_listing_id(&self) -> Option<ProductListingId> {
        self.content.product_listing_id()
    }
    pub fn mark_seen(&mut self, seen: bool) {
        self.seen = seen;
    }
}

#[derive(Debug, thiserror::Error)]
#[error("persisted notification state is invalid")]
pub struct RehydrateNotificationError;

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedNotificationState {
    pub notification_id: NotificationId,
    pub user_id: UserId,
    pub content: NotificationContent,
    pub seen: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationContent {
    Watchlist {
        origin_event_id: EventId,
        product_listing_id: ProductListingId,
        snapshot: ProductListingNotificationSnapshot,
        change: NotificationWatchlistChange,
    },
    SearchFilter {
        origin_event_id: EventId,
        product_listing_id: ProductListingId,
        user_search_filter_id: UserSearchFilterId,
        snapshot: ProductListingNotificationSnapshot,
        user_search_filter_name: UserSearchFilterName,
    },
    PartnershipApplication {
        partnership_application_id: PartnershipApplicationId,
        snapshot: PartnershipApplicationNotificationSnapshot,
        decision: PartnershipApplicationDecision,
    },
}

impl NotificationContent {
    pub fn kind(&self) -> NotificationKind {
        match self {
            Self::Watchlist {
                change: NotificationWatchlistChange::PriceChange { .. },
                ..
            } => NotificationKind::WatchlistPriceChanged,
            Self::Watchlist {
                change: NotificationWatchlistChange::AvailabilityChange { .. },
                ..
            } => NotificationKind::WatchlistAvailabilityChanged,
            Self::SearchFilter { .. } => NotificationKind::SearchFilterMatch,
            Self::PartnershipApplication {
                decision: PartnershipApplicationDecision::Approved,
                ..
            } => NotificationKind::PartnershipApplicationApproved,
            Self::PartnershipApplication {
                decision: PartnershipApplicationDecision::Rejected,
                ..
            } => NotificationKind::PartnershipApplicationRejected,
        }
    }

    pub fn origin_event_id(&self) -> Option<EventId> {
        match self {
            Self::Watchlist {
                origin_event_id, ..
            }
            | Self::SearchFilter {
                origin_event_id, ..
            } => Some(*origin_event_id),
            Self::PartnershipApplication { .. } => None,
        }
    }

    pub fn product_listing_id(&self) -> Option<ProductListingId> {
        match self {
            Self::Watchlist {
                product_listing_id, ..
            }
            | Self::SearchFilter {
                product_listing_id, ..
            } => Some(*product_listing_id),
            Self::PartnershipApplication { .. } => None,
        }
    }

    pub fn localized(
        self,
        preferred_languages: &[localization::Language],
    ) -> LocalizedNotificationContent {
        match self {
            Self::Watchlist {
                product_listing_id,
                snapshot,
                change,
                ..
            } => LocalizedNotificationContent::Watchlist {
                product_listing_id,
                snapshot: snapshot.localized(preferred_languages),
                change: change.localized(),
            },
            Self::SearchFilter {
                product_listing_id,
                snapshot,
                user_search_filter_id,
                user_search_filter_name,
                ..
            } => LocalizedNotificationContent::SearchFilter {
                product_listing_id,
                snapshot: snapshot.localized(preferred_languages),
                user_search_filter_id,
                user_search_filter_name,
            },
            Self::PartnershipApplication {
                partnership_application_id,
                snapshot,
                decision,
            } => LocalizedNotificationContent::PartnershipApplication {
                partnership_application_id,
                snapshot,
                decision,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingNotificationSnapshot {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub listing_source_slug_id: ListingSourceSlugId,
    pub product_listing_title_slug_id: ProductListingSlugId,
    pub listing_source_name: ListingSourceName,
    pub title: Option<HashMap<localization::Language, Title>>,
    pub image: Option<Url>,
    pub content_policy: Option<ContentPolicyDecision>,
    pub url: Url,
    pub view_url: Url,
}

impl ProductListingNotificationSnapshot {
    fn localized(
        self,
        preferred_languages: &[localization::Language],
    ) -> LocalizedProductListingNotificationSnapshot {
        LocalizedProductListingNotificationSnapshot {
            listing_source_id: self.listing_source_id,
            source_listing_id: self.source_listing_id,
            listing_source_slug_id: self.listing_source_slug_id,
            product_listing_title_slug_id: self.product_listing_title_slug_id,
            listing_source_name: self.listing_source_name,
            title: self
                .title
                .and_then(|titles| localization::Language::resolve(preferred_languages, titles)),
            image: self.image,
            content_policy: self.content_policy,
            url: self.url,
            view_url: self.view_url,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartnershipApplicationNotificationSnapshot {
    pub party_name: PartyName,
    pub listing_source_name: ListingSourceName,
    pub image: Option<Url>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartnershipApplicationDecision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotificationWatchlistChange {
    PriceChange {
        old_price: Option<ProductListingPrice>,
        new_price: Option<ProductListingPrice>,
    },
    AvailabilityChange {
        old_availability: Option<ListingAvailability>,
        new_availability: Option<ListingAvailability>,
    },
}

impl NotificationWatchlistChange {
    fn localized(self) -> LocalizedNotificationWatchlistChange {
        match self {
            Self::PriceChange {
                old_price,
                new_price,
            } => LocalizedNotificationWatchlistChange::PriceChange {
                old_price,
                new_price,
            },
            Self::AvailabilityChange {
                old_availability,
                new_availability,
            } => LocalizedNotificationWatchlistChange::AvailabilityChange {
                old_availability,
                new_availability,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalizedNotificationContent {
    Watchlist {
        product_listing_id: ProductListingId,
        snapshot: LocalizedProductListingNotificationSnapshot,
        change: LocalizedNotificationWatchlistChange,
    },
    SearchFilter {
        product_listing_id: ProductListingId,
        snapshot: LocalizedProductListingNotificationSnapshot,
        user_search_filter_id: UserSearchFilterId,
        user_search_filter_name: UserSearchFilterName,
    },
    PartnershipApplication {
        partnership_application_id: PartnershipApplicationId,
        snapshot: PartnershipApplicationNotificationSnapshot,
        decision: PartnershipApplicationDecision,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalizedProductListingNotificationSnapshot {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub listing_source_slug_id: ListingSourceSlugId,
    pub product_listing_title_slug_id: ProductListingSlugId,
    pub listing_source_name: ListingSourceName,
    pub title: Option<Localized<localization::Language, Title>>,
    pub image: Option<Url>,
    pub content_policy: Option<ContentPolicyDecision>,
    pub url: Url,
    pub view_url: Url,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalizedNotificationWatchlistChange {
    PriceChange {
        old_price: Option<ProductListingPrice>,
        new_price: Option<ProductListingPrice>,
    },
    AvailabilityChange {
        old_availability: Option<ListingAvailability>,
        new_availability: Option<ListingAvailability>,
    },
}

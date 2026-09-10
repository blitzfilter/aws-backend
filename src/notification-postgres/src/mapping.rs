use application::error::{BoxError, box_error};
use domain_primitives::event_id::EventId;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use money::{Currency, MonetaryAmount, Price};
use notification_core::{
    notification::{
        Notification, NotificationContent, NotificationWatchlistChange,
        PartnershipApplicationDecision, PartnershipApplicationNotificationSnapshot,
        ProductListingNotificationSnapshot, RehydratedNotificationState,
    },
    notification_id::NotificationId,
    notification_kind::NotificationKind,
};
use partnership_core::partnership_application_id::PartnershipApplicationId;
use party_core::party_name::PartyName;
use product_listing_core::{
    content_policy::{ContentPolicyDecision, SensitiveContentCategory},
    listing_availability::ListingAvailability,
    product_listing_id::ProductListingId,
    product_listing_price::ProductListingPrice,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use search_filter_core::{
    user_search_filter_id::UserSearchFilterId, user_search_filter_name::UserSearchFilterName,
};
use serde::{Deserialize, Serialize};

use std::collections::{HashMap, HashSet};
use strum::IntoEnumIterator;
use time::OffsetDateTime;
use url::Url;
use user_core::user_id::UserId;

pub(crate) const PAYLOAD_VERSION: i16 = 1;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct NotificationRow {
    pub(crate) notification_id: uuid::Uuid,
    pub(crate) user_id: uuid::Uuid,
    pub(crate) kind: String,
    pub(crate) origin_event_id: Option<uuid::Uuid>,
    pub(crate) product_listing_id: Option<uuid::Uuid>,
    pub(crate) user_search_filter_id: Option<uuid::Uuid>,
    pub(crate) partnership_application_id: Option<uuid::Uuid>,
    pub(crate) payload_version: i16,
    pub(crate) payload: serde_json::Value,
    pub(crate) seen: bool,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum NotificationMappingError {
    #[error("unknown notification kind {0}")]
    UnknownKind(String),
    #[error("unknown notification title language {0}")]
    UnknownLanguage(String),
    #[error("notification title contains duplicate language {0}")]
    DuplicateTitleLanguage(String),
    #[error("unsupported notification payload version {0}")]
    UnsupportedPayloadVersion(i16),
    #[error("notification payload serialization failed")]
    PayloadSerialization(#[source] serde_json::Error),
    #[error("notification payload is invalid")]
    InvalidPayload(#[source] serde_json::Error),
    #[error("notification payload contains an invalid value")]
    InvalidPayloadValue,
    #[error("notification content policy is invalid")]
    InvalidContentPolicy,
    #[error("notification source columns do not match its kind")]
    SourceShapeMismatch,
    #[error("notification kind does not match its payload")]
    KindPayloadMismatch,
    #[error("notification contains an invalid persisted object ID")]
    InvalidObjectId(#[source] domain_primitives::object_id::ObjectIdError),
    #[error("notification rehydration failed")]
    Rehydrate(#[source] notification_core::notification::RehydrateNotificationError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum NotificationPayloadV1 {
    Watchlist {
        snapshot: ProductListingNotificationSnapshotV1,
        change: NotificationWatchlistChangeV1,
    },
    SearchFilter {
        snapshot: ProductListingNotificationSnapshotV1,
        user_search_filter_name: UserSearchFilterName,
    },
    PartnershipApplication {
        snapshot: PartnershipApplicationNotificationSnapshotV1,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalizedTitleV1 {
    language: String,
    title: Title,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum PersistedCurrency {
    Eur,
    Gbp,
    Usd,
    Aud,
    Cad,
    Nzd,
    Cny,
    Brl,
    Pln,
    Try,
    Jpy,
    Czk,
    Rub,
    Aed,
    Sar,
    Hkd,
    Sgd,
    Chf,
    Zar,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum PersistedProductListingPrice {
    Monetary {
        currency: PersistedCurrency,
        amount: u64,
    },
    OnRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedContentPolicyDecision {
    decision: String,
    category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductListingNotificationSnapshotV1 {
    #[serde(
        serialize_with = "serialize_listing_source_id",
        deserialize_with = "deserialize_listing_source_id"
    )]
    listing_source_id: ListingSourceId,
    #[serde(
        serialize_with = "serialize_source_listing_id",
        deserialize_with = "deserialize_source_listing_id"
    )]
    source_listing_id: SourceListingId,
    #[serde(
        serialize_with = "serialize_listing_source_slug_id",
        deserialize_with = "deserialize_listing_source_slug_id"
    )]
    listing_source_slug_id: ListingSourceSlugId,
    product_listing_title_slug_id: ProductListingSlugId,
    #[serde(
        serialize_with = "serialize_listing_source_name",
        deserialize_with = "deserialize_listing_source_name"
    )]
    listing_source_name: ListingSourceName,
    title: Option<Vec<LocalizedTitleV1>>,
    image: Option<Url>,
    content_policy: Option<PersistedContentPolicyDecision>,
    url: Url,
    view_url: Url,
}

fn serialize_listing_source_id<S>(
    listing_source_id: &ListingSourceId,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&listing_source_id.as_uuid().to_string())
}

fn deserialize_listing_source_id<'de, D>(deserializer: D) -> Result<ListingSourceId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    let uuid = uuid::Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
    if uuid.to_string() != value {
        return Err(serde::de::Error::custom(
            "persisted listing source UUID must use canonical hyphenated lowercase text",
        ));
    }
    ListingSourceId::try_from(uuid).map_err(serde::de::Error::custom)
}

fn serialize_source_listing_id<S>(
    source_listing_id: &SourceListingId,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(source_listing_id.as_ref())
}

fn deserialize_source_listing_id<'de, D>(deserializer: D) -> Result<SourceListingId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    SourceListingId::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn serialize_listing_source_slug_id<S>(
    listing_source_slug_id: &ListingSourceSlugId,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(listing_source_slug_id.as_ref())
}

fn deserialize_listing_source_slug_id<'de, D>(
    deserializer: D,
) -> Result<ListingSourceSlugId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    ListingSourceSlugId::raw(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
}

fn serialize_listing_source_name<S>(
    listing_source_name: &ListingSourceName,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(listing_source_name.as_ref())
}

fn deserialize_listing_source_name<'de, D>(deserializer: D) -> Result<ListingSourceName, D::Error>
where
    D: serde::Deserializer<'de>,
{
    ListingSourceName::try_from(String::deserialize(deserializer)?)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum NotificationWatchlistChangeV1 {
    PriceChange {
        old_price: Option<PersistedProductListingPrice>,
        new_price: Option<PersistedProductListingPrice>,
    },
    #[serde(rename = "AVAILABILITY_CHANGE")]
    AvailabilityChange {
        #[serde(
            serialize_with = "serialize_optional_listing_availability",
            deserialize_with = "deserialize_optional_listing_availability"
        )]
        old_availability: Option<ListingAvailability>,
        #[serde(
            serialize_with = "serialize_optional_listing_availability",
            deserialize_with = "deserialize_optional_listing_availability"
        )]
        new_availability: Option<ListingAvailability>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartnershipApplicationNotificationSnapshotV1 {
    party_name: String,
    listing_source_name: String,
    image: Option<Url>,
}

impl From<&ProductListingNotificationSnapshot> for ProductListingNotificationSnapshotV1 {
    fn from(snapshot: &ProductListingNotificationSnapshot) -> Self {
        Self {
            listing_source_id: snapshot.listing_source_id,
            source_listing_id: snapshot.source_listing_id.clone(),
            listing_source_slug_id: snapshot.listing_source_slug_id.clone(),
            product_listing_title_slug_id: snapshot.product_listing_title_slug_id.clone(),
            listing_source_name: snapshot.listing_source_name.clone(),
            title: snapshot.title.as_ref().map(|titles| {
                titles
                    .iter()
                    .map(|(language, title)| LocalizedTitleV1 {
                        language: language.as_str().to_owned(),
                        title: title.clone(),
                    })
                    .collect()
            }),
            image: snapshot.image.clone(),
            content_policy: snapshot.content_policy.map(Into::into),
            url: snapshot.url.clone(),
            view_url: snapshot.view_url.clone(),
        }
    }
}

impl TryFrom<ProductListingNotificationSnapshotV1> for ProductListingNotificationSnapshot {
    type Error = NotificationMappingError;

    fn try_from(snapshot: ProductListingNotificationSnapshotV1) -> Result<Self, Self::Error> {
        let title = snapshot
            .title
            .map(|titles| {
                let mut seen_languages = HashSet::new();
                titles
                    .into_iter()
                    .map(|title| {
                        let language = parse_language(&title.language)?;
                        if !seen_languages.insert(language) {
                            return Err(NotificationMappingError::DuplicateTitleLanguage(
                                title.language,
                            ));
                        }
                        Ok((language, title.title))
                    })
                    .collect::<Result<HashMap<_, _>, NotificationMappingError>>()
            })
            .transpose()?;
        Ok(Self {
            listing_source_id: snapshot.listing_source_id,
            source_listing_id: snapshot.source_listing_id,
            listing_source_slug_id: snapshot.listing_source_slug_id,
            product_listing_title_slug_id: snapshot.product_listing_title_slug_id,
            listing_source_name: snapshot.listing_source_name,
            title,
            image: snapshot.image,
            content_policy: snapshot.content_policy.map(TryInto::try_into).transpose()?,
            url: snapshot.url,
            view_url: snapshot.view_url,
        })
    }
}

impl From<ContentPolicyDecision> for PersistedContentPolicyDecision {
    fn from(decision: ContentPolicyDecision) -> Self {
        match decision {
            ContentPolicyDecision::Allowed => Self {
                decision: decision.as_str().to_owned(),
                category: None,
            },
            ContentPolicyDecision::RequiresConsent(category) => Self {
                decision: decision.as_str().to_owned(),
                category: Some(category.as_str().to_owned()),
            },
        }
    }
}

impl TryFrom<PersistedContentPolicyDecision> for ContentPolicyDecision {
    type Error = NotificationMappingError;

    fn try_from(value: PersistedContentPolicyDecision) -> Result<Self, Self::Error> {
        match (value.decision.as_str(), value.category.as_deref()) {
            ("ALLOWED", None) => Ok(Self::Allowed),
            ("REQUIRES_CONSENT", Some(category)) => SensitiveContentCategory::from_code(category)
                .map(Self::RequiresConsent)
                .ok_or(NotificationMappingError::InvalidContentPolicy),
            _ => Err(NotificationMappingError::InvalidContentPolicy),
        }
    }
}

impl From<&NotificationWatchlistChange> for NotificationWatchlistChangeV1 {
    fn from(change: &NotificationWatchlistChange) -> Self {
        match change {
            NotificationWatchlistChange::PriceChange {
                old_price,
                new_price,
            } => Self::PriceChange {
                old_price: old_price.map(price_data_from_price),
                new_price: new_price.map(price_data_from_price),
            },
            NotificationWatchlistChange::AvailabilityChange {
                old_availability,
                new_availability,
            } => Self::AvailabilityChange {
                old_availability: *old_availability,
                new_availability: *new_availability,
            },
        }
    }
}

impl From<NotificationWatchlistChangeV1> for NotificationWatchlistChange {
    fn from(change: NotificationWatchlistChangeV1) -> Self {
        match change {
            NotificationWatchlistChangeV1::PriceChange {
                old_price,
                new_price,
            } => Self::PriceChange {
                old_price: old_price.map(price_from_data),
                new_price: new_price.map(price_from_data),
            },
            NotificationWatchlistChangeV1::AvailabilityChange {
                old_availability,
                new_availability,
            } => Self::AvailabilityChange {
                old_availability,
                new_availability,
            },
        }
    }
}

impl From<&PartnershipApplicationNotificationSnapshot>
    for PartnershipApplicationNotificationSnapshotV1
{
    fn from(snapshot: &PartnershipApplicationNotificationSnapshot) -> Self {
        Self {
            party_name: snapshot.party_name.to_string(),
            listing_source_name: snapshot.listing_source_name.to_string(),
            image: snapshot.image.clone(),
        }
    }
}

impl TryFrom<PartnershipApplicationNotificationSnapshotV1>
    for PartnershipApplicationNotificationSnapshot
{
    type Error = NotificationMappingError;

    fn try_from(
        snapshot: PartnershipApplicationNotificationSnapshotV1,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            party_name: PartyName::try_from(snapshot.party_name)
                .map_err(|_| NotificationMappingError::InvalidPayloadValue)?,
            listing_source_name: ListingSourceName::try_from(snapshot.listing_source_name)
                .map_err(|_| NotificationMappingError::InvalidPayloadValue)?,
            image: snapshot.image,
        })
    }
}

pub(crate) struct NotificationWriteValues {
    pub(crate) notification_id: uuid::Uuid,
    pub(crate) user_id: uuid::Uuid,
    pub(crate) kind: &'static str,
    pub(crate) origin_event_id: Option<uuid::Uuid>,
    pub(crate) product_listing_id: Option<uuid::Uuid>,
    pub(crate) user_search_filter_id: Option<uuid::Uuid>,
    pub(crate) partnership_application_id: Option<uuid::Uuid>,
    pub(crate) payload: serde_json::Value,
}

impl TryFrom<&Notification> for NotificationWriteValues {
    type Error = NotificationMappingError;

    fn try_from(notification: &Notification) -> Result<Self, Self::Error> {
        let (
            origin_event_id,
            product_listing_id,
            user_search_filter_id,
            partnership_application_id,
            payload,
        ) = match notification.content() {
            NotificationContent::Watchlist {
                origin_event_id,
                product_listing_id,
                snapshot,
                change,
            } => (
                Some(origin_event_id.into_uuid()),
                Some(product_listing_id.into_uuid()),
                None,
                None,
                NotificationPayloadV1::Watchlist {
                    snapshot: snapshot.into(),
                    change: change.into(),
                },
            ),
            NotificationContent::SearchFilter {
                origin_event_id,
                product_listing_id,
                user_search_filter_id,
                snapshot,
                user_search_filter_name,
            } => (
                Some(origin_event_id.into_uuid()),
                Some(product_listing_id.into_uuid()),
                Some(user_search_filter_id.into_uuid()),
                None,
                NotificationPayloadV1::SearchFilter {
                    snapshot: snapshot.into(),
                    user_search_filter_name: user_search_filter_name.clone(),
                },
            ),
            NotificationContent::PartnershipApplication {
                partnership_application_id,
                snapshot,
                ..
            } => (
                None,
                None,
                None,
                Some(partnership_application_id.into_uuid()),
                NotificationPayloadV1::PartnershipApplication {
                    snapshot: snapshot.into(),
                },
            ),
        };
        let payload = serde_json::to_value(payload)
            .map_err(NotificationMappingError::PayloadSerialization)?;
        Ok(Self {
            notification_id: notification.notification_id().into_uuid(),
            user_id: notification.user_id().into_uuid(),
            kind: notification.kind().as_str(),
            origin_event_id,
            product_listing_id,
            user_search_filter_id,
            partnership_application_id,
            payload,
        })
    }
}

impl TryFrom<NotificationRow> for Notification {
    type Error = NotificationMappingError;

    fn try_from(row: NotificationRow) -> Result<Self, Self::Error> {
        if row.payload_version != PAYLOAD_VERSION {
            return Err(NotificationMappingError::UnsupportedPayloadVersion(
                row.payload_version,
            ));
        }
        let kind = parse_kind(&row.kind)?;
        let payload = serde_json::from_value::<NotificationPayloadV1>(row.payload)
            .map_err(NotificationMappingError::InvalidPayload)?;
        let content = match (
            kind,
            payload,
            row.origin_event_id,
            row.product_listing_id,
            row.user_search_filter_id,
            row.partnership_application_id,
        ) {
            (
                NotificationKind::WatchlistPriceChanged
                | NotificationKind::WatchlistAvailabilityChanged,
                NotificationPayloadV1::Watchlist { snapshot, change },
                Some(origin_event_id),
                Some(product_listing_id),
                None,
                None,
            ) => {
                let change: NotificationWatchlistChange = change.into();
                if change_kind(&change) != kind {
                    return Err(NotificationMappingError::KindPayloadMismatch);
                }
                NotificationContent::Watchlist {
                    origin_event_id: EventId::try_from(origin_event_id)
                        .map_err(NotificationMappingError::InvalidObjectId)?,
                    product_listing_id: ProductListingId::try_from(product_listing_id)
                        .map_err(NotificationMappingError::InvalidObjectId)?,
                    snapshot: snapshot.try_into()?,
                    change,
                }
            }
            (
                NotificationKind::SearchFilterMatch,
                NotificationPayloadV1::SearchFilter {
                    snapshot,
                    user_search_filter_name,
                },
                Some(origin_event_id),
                Some(product_listing_id),
                Some(user_search_filter_id),
                None,
            ) => NotificationContent::SearchFilter {
                origin_event_id: EventId::try_from(origin_event_id)
                    .map_err(NotificationMappingError::InvalidObjectId)?,
                product_listing_id: ProductListingId::try_from(product_listing_id)
                    .map_err(NotificationMappingError::InvalidObjectId)?,
                user_search_filter_id: UserSearchFilterId::try_from(user_search_filter_id)
                    .map_err(NotificationMappingError::InvalidObjectId)?,
                snapshot: snapshot.try_into()?,
                user_search_filter_name,
            },
            (
                NotificationKind::PartnershipApplicationApproved
                | NotificationKind::PartnershipApplicationRejected,
                NotificationPayloadV1::PartnershipApplication { snapshot },
                None,
                None,
                None,
                Some(partnership_application_id),
            ) => NotificationContent::PartnershipApplication {
                partnership_application_id: PartnershipApplicationId::try_from(
                    partnership_application_id,
                )
                .map_err(NotificationMappingError::InvalidObjectId)?,
                snapshot: snapshot.try_into()?,
                decision: if kind == NotificationKind::PartnershipApplicationApproved {
                    PartnershipApplicationDecision::Approved
                } else {
                    PartnershipApplicationDecision::Rejected
                },
            },
            (_, NotificationPayloadV1::Watchlist { .. }, _, _, _, _)
            | (_, NotificationPayloadV1::SearchFilter { .. }, _, _, _, _)
            | (_, NotificationPayloadV1::PartnershipApplication { .. }, _, _, _, _) => {
                return Err(NotificationMappingError::SourceShapeMismatch);
            }
        };
        Notification::rehydrate(RehydratedNotificationState {
            notification_id: NotificationId::try_from(row.notification_id)
                .map_err(NotificationMappingError::InvalidObjectId)?,
            user_id: UserId::try_from(row.user_id)
                .map_err(NotificationMappingError::InvalidObjectId)?,
            content,
            seen: row.seen,
        })
        .map_err(NotificationMappingError::Rehydrate)
    }
}

fn price_data_from_price(price: ProductListingPrice) -> PersistedProductListingPrice {
    let ProductListingPrice::Monetary(price) = price else {
        return PersistedProductListingPrice::OnRequest;
    };
    let currency = match price.currency {
        Currency::Eur => PersistedCurrency::Eur,
        Currency::Gbp => PersistedCurrency::Gbp,
        Currency::Usd => PersistedCurrency::Usd,
        Currency::Aud => PersistedCurrency::Aud,
        Currency::Cad => PersistedCurrency::Cad,
        Currency::Nzd => PersistedCurrency::Nzd,
        Currency::Cny => PersistedCurrency::Cny,
        Currency::Brl => PersistedCurrency::Brl,
        Currency::Pln => PersistedCurrency::Pln,
        Currency::Try => PersistedCurrency::Try,
        Currency::Jpy => PersistedCurrency::Jpy,
        Currency::Czk => PersistedCurrency::Czk,
        Currency::Rub => PersistedCurrency::Rub,
        Currency::Aed => PersistedCurrency::Aed,
        Currency::Sar => PersistedCurrency::Sar,
        Currency::Hkd => PersistedCurrency::Hkd,
        Currency::Sgd => PersistedCurrency::Sgd,
        Currency::Chf => PersistedCurrency::Chf,
        Currency::Zar => PersistedCurrency::Zar,
    };
    PersistedProductListingPrice::Monetary {
        currency,
        amount: price.monetary_amount.into(),
    }
}

fn price_from_data(price: PersistedProductListingPrice) -> ProductListingPrice {
    let PersistedProductListingPrice::Monetary { currency, amount } = price else {
        return ProductListingPrice::OnRequest;
    };
    let currency = match currency {
        PersistedCurrency::Eur => Currency::Eur,
        PersistedCurrency::Gbp => Currency::Gbp,
        PersistedCurrency::Usd => Currency::Usd,
        PersistedCurrency::Aud => Currency::Aud,
        PersistedCurrency::Cad => Currency::Cad,
        PersistedCurrency::Nzd => Currency::Nzd,
        PersistedCurrency::Cny => Currency::Cny,
        PersistedCurrency::Brl => Currency::Brl,
        PersistedCurrency::Pln => Currency::Pln,
        PersistedCurrency::Try => Currency::Try,
        PersistedCurrency::Jpy => Currency::Jpy,
        PersistedCurrency::Czk => Currency::Czk,
        PersistedCurrency::Rub => Currency::Rub,
        PersistedCurrency::Aed => Currency::Aed,
        PersistedCurrency::Sar => Currency::Sar,
        PersistedCurrency::Hkd => Currency::Hkd,
        PersistedCurrency::Sgd => Currency::Sgd,
        PersistedCurrency::Chf => Currency::Chf,
        PersistedCurrency::Zar => Currency::Zar,
    };
    ProductListingPrice::Monetary(Price::new(MonetaryAmount::from(amount), currency))
}

fn serialize_optional_listing_availability<S>(
    availability: &Option<ListingAvailability>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match availability {
        Some(availability) => serializer.serialize_some(availability.as_str()),
        None => serializer.serialize_none(),
    }
}

fn deserialize_optional_listing_availability<'de, D>(
    deserializer: D,
) -> Result<Option<ListingAvailability>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?.map_or(Ok(None), |value| {
        ListingAvailability::from_code(&value)
            .map(Some)
            .ok_or_else(|| {
                <D::Error as serde::de::Error>::custom(format!(
                    "unknown listing availability {value}"
                ))
            })
    })
}

fn parse_language(value: &str) -> Result<localization::Language, NotificationMappingError> {
    localization::Language::from_code(value)
        .ok_or_else(|| NotificationMappingError::UnknownLanguage(value.to_owned()))
}

fn parse_kind(value: &str) -> Result<NotificationKind, NotificationMappingError> {
    NotificationKind::iter()
        .find(|kind| kind.as_str() == value)
        .ok_or_else(|| NotificationMappingError::UnknownKind(value.to_owned()))
}

fn change_kind(change: &NotificationWatchlistChange) -> NotificationKind {
    match change {
        NotificationWatchlistChange::PriceChange { .. } => NotificationKind::WatchlistPriceChanged,
        NotificationWatchlistChange::AvailabilityChange { .. } => {
            NotificationKind::WatchlistAvailabilityChanged
        }
    }
}

pub(crate) fn mapping_error(error: NotificationMappingError) -> BoxError {
    box_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use money::{Currency, MonetaryAmount};
    use notification_core::notification::{
        PartnershipApplicationDecision, PartnershipApplicationNotificationSnapshot,
    };
    use notification_core::notification_id::NotificationId;
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use party_core::party_name::PartyName;
    use product_listing_core::source_listing_id::SourceListingId;
    use time::OffsetDateTime;
    use user_core::user_id::UserId;

    #[test]
    fn should_parse_each_canonical_persisted_kind() {
        for expected in NotificationKind::iter() {
            assert!(matches!(
                parse_kind(expected.as_str()),
                Ok(actual) if actual == expected
            ));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_persisted_kind() {
        assert!(matches!(
            parse_kind("watchlist_price_changed"),
            Err(NotificationMappingError::UnknownKind(value)) if value == "watchlist_price_changed"
        ));
    }

    #[test]
    fn should_serialize_product_listing_snapshot_with_listing_source_vocabulary()
    -> Result<(), Box<dyn std::error::Error>> {
        let listing_source_id = ListingSourceId::new();
        let listing_source_uuid = listing_source_id.as_uuid().to_string();
        let snapshot = ProductListingNotificationSnapshot {
            listing_source_id,
            source_listing_id: SourceListingId::try_from("source-listing-42")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            listing_source_slug_id: ListingSourceSlugId::raw("northwind-source")?,
            product_listing_title_slug_id: ProductListingSlugId::raw("rare-vase-000000")?,
            listing_source_name: ListingSourceName::try_from("Northwind Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            title: None,
            image: None,
            content_policy: None,
            url: Url::parse("https://source.example/listings/42")?,
            view_url: Url::parse("https://aura.example/listings/rare-vase")?,
        };

        let persisted = ProductListingNotificationSnapshotV1::from(&snapshot);

        assert_eq!(
            serde_json::json!({
                "listing_source_id": listing_source_uuid,
                "source_listing_id": "source-listing-42",
                "listing_source_slug_id": "northwind-source",
                "product_listing_title_slug_id": "rare-vase-000000",
                "listing_source_name": "Northwind Source",
                "title": null,
                "image": null,
                "content_policy": null,
                "url": "https://source.example/listings/42",
                "view_url": "https://aura.example/listings/rare-vase",
            }),
            serde_json::to_value(&persisted)?
        );
        assert_eq!(
            snapshot,
            ProductListingNotificationSnapshot::try_from(persisted)?
        );

        Ok(())
    }

    #[test]
    fn should_reject_invalid_persisted_listing_source_name() {
        let result =
            serde_json::from_value::<ProductListingNotificationSnapshotV1>(serde_json::json!({
                "listing_source_id": ListingSourceId::new().as_uuid().to_string(),
                "source_listing_id": "source-listing-42",
                "listing_source_slug_id": "northwind-source",
                "product_listing_title_slug_id": "rare-vase-000000",
                "listing_source_name": "\u{2003}",
                "title": null,
                "image": null,
                "content_policy": null,
                "url": "https://source.example/listings/42",
                "view_url": "https://aura.example/listings/rare-vase"
            }));

        assert!(result.is_err());
    }

    #[test]
    fn should_reject_noncanonical_persisted_listing_source_id_text() {
        let canonical = "01890a5d-ac96-774b-bf1d-d5586c639f75";
        let noncanonical_values = [
            canonical.to_uppercase(),
            canonical.replace('-', ""),
            format!("{{{canonical}}}"),
            format!("urn:uuid:{canonical}"),
        ];

        for listing_source_id in noncanonical_values {
            assert!(uuid::Uuid::parse_str(&listing_source_id).is_ok());
            let result =
                serde_json::from_value::<ProductListingNotificationSnapshotV1>(serde_json::json!({
                    "listing_source_id": listing_source_id,
                    "source_listing_id": "source-listing-42",
                    "listing_source_slug_id": "northwind-source",
                    "product_listing_title_slug_id": "rare-vase-000000",
                    "listing_source_name": "Northwind Source",
                    "title": null,
                    "image": null,
                    "content_policy": null,
                    "url": "https://source.example/listings/42",
                    "view_url": "https://aura.example/listings/rare-vase"
                }));

            assert!(matches!(
                result,
                Err(error) if error
                    .to_string()
                    .contains("canonical hyphenated lowercase text")
            ));
        }
    }

    #[test]
    fn should_reject_v4_persisted_listing_source_id() {
        let result =
            serde_json::from_value::<ProductListingNotificationSnapshotV1>(serde_json::json!({
                "listing_source_id": uuid::Uuid::new_v4().to_string(),
                "source_listing_id": "source-listing-42",
                "listing_source_slug_id": "northwind-source",
                "product_listing_title_slug_id": "rare-vase-000000",
                "listing_source_name": "Northwind Source",
                "title": null,
                "image": null,
                "content_policy": null,
                "url": "https://source.example/listings/42",
                "view_url": "https://aura.example/listings/rare-vase"
            }));

        assert!(result.is_err());
    }

    #[test]
    fn should_serialize_canonical_nullable_listing_availability()
    -> Result<(), Box<dyn std::error::Error>> {
        let change = NotificationWatchlistChange::AvailabilityChange {
            old_availability: Some(ListingAvailability::Available),
            new_availability: None,
        };
        let persisted = NotificationWatchlistChangeV1::from(&change);

        assert_eq!(
            serde_json::json!({
                "type": "AVAILABILITY_CHANGE",
                "old_availability": "AVAILABLE",
                "new_availability": null,
            }),
            serde_json::to_value(persisted)?
        );
        Ok(())
    }

    #[test]
    fn should_deserialize_nullable_listing_availability() -> Result<(), serde_json::Error> {
        let change = serde_json::from_value::<NotificationWatchlistChangeV1>(serde_json::json!({
            "type": "AVAILABILITY_CHANGE",
            "old_availability": null,
            "new_availability": "IN_STOCK",
        }))?;

        assert!(matches!(
            change,
            NotificationWatchlistChangeV1::AvailabilityChange {
                old_availability: None,
                new_availability: Some(ListingAvailability::InStock),
            }
        ));
        Ok(())
    }

    #[test]
    fn should_reject_unknown_listing_availability() {
        assert!(
            serde_json::from_value::<NotificationWatchlistChangeV1>(serde_json::json!({
                "type": "AVAILABILITY_CHANGE",
                "old_availability": null,
                "new_availability": "UNKNOWN",
            }))
            .is_err()
        );
    }

    #[test]
    fn should_reject_unknown_content_policy_json_fields() {
        assert!(
            serde_json::from_value::<PersistedContentPolicyDecision>(serde_json::json!({
                "decision": "ALLOWED",
                "category": null,
                "unexpected": true,
            }))
            .is_err()
        );
    }

    #[test]
    fn should_round_trip_partnership_application_notification_write_values()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification = Notification::new(
            NotificationId::new(),
            UserId::new(),
            NotificationContent::PartnershipApplication {
                partnership_application_id: PartnershipApplicationId::new(),
                snapshot: PartnershipApplicationNotificationSnapshot {
                    party_name: PartyName::try_from("Northwind Antiques")?,
                    listing_source_name: ListingSourceName::try_from("Northwind Source")
                        .unwrap_or_else(|error| {
                            panic!("invalid test listing source name: {error}")
                        }),
                    image: None,
                },
                decision: PartnershipApplicationDecision::Approved,
            },
        );
        let values = NotificationWriteValues::try_from(&notification)?;

        let restored = Notification::try_from(NotificationRow {
            notification_id: values.notification_id,
            user_id: values.user_id,
            kind: values.kind.to_owned(),
            origin_event_id: values.origin_event_id,
            product_listing_id: values.product_listing_id,
            user_search_filter_id: values.user_search_filter_id,
            partnership_application_id: values.partnership_application_id,
            payload_version: PAYLOAD_VERSION,
            payload: values.payload,
            seen: false,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })?;

        assert_eq!(notification, restored);
        Ok(())
    }

    #[test]
    fn should_reject_v4_notification_id_from_storage_row() -> Result<(), Box<dyn std::error::Error>>
    {
        let notification = Notification::new(
            NotificationId::new(),
            UserId::new(),
            NotificationContent::PartnershipApplication {
                partnership_application_id: PartnershipApplicationId::new(),
                snapshot: PartnershipApplicationNotificationSnapshot {
                    party_name: PartyName::try_from("Northwind Antiques")?,
                    listing_source_name: ListingSourceName::try_from("Northwind Source")?,
                    image: None,
                },
                decision: PartnershipApplicationDecision::Approved,
            },
        );
        let values = NotificationWriteValues::try_from(&notification)?;
        let result = Notification::try_from(NotificationRow {
            notification_id: uuid::Uuid::new_v4(),
            user_id: values.user_id,
            kind: values.kind.to_owned(),
            origin_event_id: values.origin_event_id,
            product_listing_id: values.product_listing_id,
            user_search_filter_id: values.user_search_filter_id,
            partnership_application_id: values.partnership_application_id,
            payload_version: PAYLOAD_VERSION,
            payload: values.payload,
            seen: false,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        });

        assert!(matches!(
            result,
            Err(NotificationMappingError::InvalidObjectId(
                domain_primitives::object_id::ObjectIdError::UnsupportedUuidVersion { actual: 4 }
            ))
        ));
        Ok(())
    }

    #[test]
    fn should_reject_partnership_application_notification_with_invalid_source_shape()
    -> Result<(), Box<dyn std::error::Error>> {
        let notification = Notification::new(
            NotificationId::new(),
            UserId::new(),
            NotificationContent::PartnershipApplication {
                partnership_application_id: PartnershipApplicationId::new(),
                snapshot: PartnershipApplicationNotificationSnapshot {
                    party_name: PartyName::try_from("Northwind Antiques")?,
                    listing_source_name: ListingSourceName::try_from("Northwind Source")
                        .unwrap_or_else(|error| {
                            panic!("invalid test listing source name: {error}")
                        }),
                    image: None,
                },
                decision: PartnershipApplicationDecision::Rejected,
            },
        );
        let values = NotificationWriteValues::try_from(&notification)?;

        let result = Notification::try_from(NotificationRow {
            notification_id: values.notification_id,
            user_id: values.user_id,
            kind: values.kind.to_owned(),
            origin_event_id: values.origin_event_id,
            product_listing_id: values.product_listing_id,
            user_search_filter_id: values.user_search_filter_id,
            partnership_application_id: None,
            payload_version: PAYLOAD_VERSION,
            payload: values.payload,
            seen: false,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        });

        assert!(matches!(
            result,
            Err(NotificationMappingError::SourceShapeMismatch)
        ));
        Ok(())
    }

    #[test]
    fn should_serialize_source_currency_explicitly() -> Result<(), Box<dyn std::error::Error>> {
        let change = NotificationWatchlistChange::PriceChange {
            old_price: Some(Price::new(MonetaryAmount::from(1000_u64), Currency::Eur).into()),
            new_price: Some(Price::new(MonetaryAmount::from(900_u64), Currency::Eur).into()),
        };
        let persisted = NotificationWatchlistChangeV1::from(&change);

        assert_eq!(
            serde_json::json!({
                "type": "PRICE_CHANGE",
                "old_price": {
                    "type": "MONETARY",
                    "currency": "EUR",
                    "amount": 1000
                },
                "new_price": {
                    "type": "MONETARY",
                    "currency": "EUR",
                    "amount": 900
                },
            }),
            serde_json::to_value(persisted)?
        );
        Ok(())
    }

    #[test]
    fn should_serialize_and_rehydrate_on_request_watchlist_prices()
    -> Result<(), Box<dyn std::error::Error>> {
        let change = NotificationWatchlistChange::PriceChange {
            old_price: Some(ProductListingPrice::OnRequest),
            new_price: None,
        };
        let persisted = NotificationWatchlistChangeV1::from(&change);
        let value = serde_json::to_value(&persisted)?;

        assert_eq!(
            serde_json::json!({
                "type": "PRICE_CHANGE",
                "old_price": { "type": "ON_REQUEST" },
                "new_price": null,
            }),
            value
        );
        assert!(matches!(
            NotificationWatchlistChange::from(persisted),
            NotificationWatchlistChange::PriceChange {
                old_price: Some(ProductListingPrice::OnRequest),
                new_price: None,
            }
        ));
        Ok(())
    }
}

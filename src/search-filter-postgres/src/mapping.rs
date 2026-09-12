use application::error::box_error;
use domain_primitives::event_id::EventId;
use domain_primitives::query::range_query::RangeQuery;
use fxrate_core::FxRateId;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_orderability::ListingOrderability;
use product_listing_core::product_listing::ProductListingPriceValuationBasis;
use product_listing_core::product_listing_id::ProductListingId;
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use user_core::user_id::UserId;

use localization::Language;
use money::Currency;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, EnhancedSearchDescriptionError, ListingAvailabilityQuery,
    ProductListingSearch,
};
use search_filter_core::{SearchFilter, SearchFilterProductListingMatch};
use search_filter_service::ports::{
    PersistedSearchFilter, PersistedSearchFilterMatch, SearchFilterIndexReadError,
    SearchFilterMatchView, SearchFilterProjection, SearchFilterReadError,
    SearchFilterRepositoryError, SearchFilterView,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::{collections::HashSet, error::Error, fmt};
use strum::IntoEnumIterator;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub(crate) const FILTER_COLUMNS: &str = "user_search_filter_id, user_id, name, notifications, state, search, embedding, created, updated, version";
pub(crate) const MATCH_COLUMNS: &str = "user_id, user_search_filter_id, product_listing_id, origin_event_id, price_valuation_basis, price_fx_rate_id, user_search_filter_name, enhanced_match_reason, feedback, created, updated";

#[derive(Debug)]
pub(crate) enum SearchFilterRowMappingError {
    NameTooLong,
    InvalidState,
    InvalidPriceMatchValuation,
    InvalidObjectId(domain_primitives::object_id::ObjectIdError),
}

impl fmt::Display for SearchFilterRowMappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NameTooLong => {
                formatter.write_str("persisted search filter name exceeds 255 characters")
            }
            Self::InvalidState => formatter.write_str("persisted search filter state is invalid"),
            Self::InvalidPriceMatchValuation => {
                formatter.write_str("persisted price match valuation is invalid")
            }
            Self::InvalidObjectId(_) => formatter.write_str("persisted object ID is invalid"),
        }
    }
}

impl Error for SearchFilterRowMappingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidObjectId(source) => Some(source),
            Self::NameTooLong | Self::InvalidState | Self::InvalidPriceMatchValuation => None,
        }
    }
}

impl From<domain_primitives::object_id::ObjectIdError> for SearchFilterRowMappingError {
    fn from(source: domain_primitives::object_id::ObjectIdError) -> Self {
        Self::InvalidObjectId(source)
    }
}

#[derive(Debug)]
pub(crate) enum ProductListingSearchJsonMappingError {
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    FormatTimestamp(time::error::Format),
    ParseTimestamp(time::error::Parse),
    EnhancedSearchDescription(EnhancedSearchDescriptionError),
    ObjectId(domain_primitives::object_id::ObjectIdError),
}

impl fmt::Display for ProductListingSearchJsonMappingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(_) => {
                formatter.write_str("search filter product search JSON serialization failed")
            }
            Self::Deserialize(_) => {
                formatter.write_str("persisted search filter product search JSON is invalid")
            }
            Self::FormatTimestamp(_) => {
                formatter.write_str("search filter product search timestamp formatting failed")
            }
            Self::ParseTimestamp(_) => {
                formatter.write_str("search filter product search timestamp is invalid")
            }
            Self::EnhancedSearchDescription(_) => {
                formatter.write_str("persisted enhanced search description is invalid")
            }
            Self::ObjectId(_) => {
                formatter.write_str("persisted product search object ID is invalid")
            }
        }
    }
}

impl Error for ProductListingSearchJsonMappingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialize(source) | Self::Deserialize(source) => Some(source),
            Self::FormatTimestamp(source) => Some(source),
            Self::ParseTimestamp(source) => Some(source),
            Self::EnhancedSearchDescription(source) => Some(source),
            Self::ObjectId(source) => Some(source),
        }
    }
}

#[derive(Debug, FromRow)]
pub(crate) struct FilterRow {
    pub user_search_filter_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub name: String,
    pub notifications: bool,
    pub state: String,
    pub search: serde_json::Value,
    pub embedding: Option<Vec<f32>>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
    pub version: i64,
}
impl FilterRow {
    pub(crate) fn into_persisted(
        self,
    ) -> Result<PersistedSearchFilter, SearchFilterRepositoryError> {
        let created = self.created;
        let updated = self.updated;
        let filter = SearchFilter::rehydrate(
            UserSearchFilterId::try_from(self.user_search_filter_id).map_err(|source| {
                SearchFilterRepositoryError::InvalidPersistedState {
                    source: box_error(SearchFilterRowMappingError::InvalidObjectId(source)),
                }
            })?,
            UserId::try_from(self.user_id).map_err(|source| {
                SearchFilterRepositoryError::InvalidPersistedState {
                    source: box_error(SearchFilterRowMappingError::InvalidObjectId(source)),
                }
            })?,
            name(self.name).map_err(|source| {
                SearchFilterRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            self.notifications,
            state(&self.state).map_err(|source| {
                SearchFilterRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            product_search_from_json(self.search).map_err(|source| {
                SearchFilterRepositoryError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            self.embedding,
        );
        Ok(PersistedSearchFilter {
            filter,
            created,
            updated,
            version: self.version,
        })
    }
    pub(crate) fn into_view(self) -> Result<SearchFilterView, SearchFilterReadError> {
        let created = self.created;
        let updated = self.updated;
        Ok(SearchFilterView {
            search_filter_id: UserSearchFilterId::try_from(self.user_search_filter_id)
                .map_err(|_| SearchFilterReadError::InvalidPersistedState)?,
            user_id: UserId::try_from(self.user_id)
                .map_err(|_| SearchFilterReadError::InvalidPersistedState)?,
            name: name(self.name).map_err(|_| SearchFilterReadError::InvalidPersistedState)?,
            notifications: self.notifications,
            state: state(&self.state).map_err(|_| SearchFilterReadError::InvalidPersistedState)?,
            search: product_search_from_json(self.search)
                .map_err(|_| SearchFilterReadError::InvalidPersistedState)?,
            embedding: self.embedding,
            created,
            updated,
        })
    }

    pub(crate) fn into_projection(
        self,
    ) -> Result<SearchFilterProjection, SearchFilterIndexReadError> {
        let source_version = self.version;
        let created = self.created;
        let updated = self.updated;
        let view = SearchFilterView {
            search_filter_id: UserSearchFilterId::try_from(self.user_search_filter_id).map_err(
                |source| SearchFilterIndexReadError::InvalidPersistedState {
                    source: box_error(source),
                },
            )?,
            user_id: UserId::try_from(self.user_id).map_err(|source| {
                SearchFilterIndexReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            name: name(self.name).map_err(|source| {
                SearchFilterIndexReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            notifications: self.notifications,
            state: state(&self.state).map_err(|source| {
                SearchFilterIndexReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            search: product_search_from_json(self.search).map_err(|source| {
                SearchFilterIndexReadError::InvalidPersistedState {
                    source: box_error(source),
                }
            })?,
            embedding: self.embedding,
            created,
            updated,
        };
        Ok(SearchFilterProjection {
            view,
            source_version,
        })
    }
}
#[derive(Debug, FromRow)]
pub(crate) struct MatchRow {
    pub user_id: uuid::Uuid,
    pub user_search_filter_id: uuid::Uuid,
    pub product_listing_id: uuid::Uuid,
    pub origin_event_id: uuid::Uuid,
    pub price_valuation_basis: Option<String>,
    pub price_fx_rate_id: Option<uuid::Uuid>,
    pub user_search_filter_name: Option<String>,
    pub enhanced_match_reason: Option<String>,
    pub feedback: Option<bool>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}
impl TryFrom<MatchRow> for PersistedSearchFilterMatch {
    type Error = SearchFilterRowMappingError;
    fn try_from(row: MatchRow) -> Result<Self, Self::Error> {
        Ok(Self {
            product_match: SearchFilterProductListingMatch {
                user_id: UserId::try_from(row.user_id)?,
                user_search_filter_id: UserSearchFilterId::try_from(row.user_search_filter_id)?,
                user_search_filter_name: row.user_search_filter_name.map(name).transpose()?,
                product_listing_id: ProductListingId::try_from(row.product_listing_id)?,
                origin_event_id: EventId::try_from(row.origin_event_id)?,
                price_match_valuation: price_match_valuation(
                    row.price_valuation_basis.as_deref(),
                    row.price_fx_rate_id,
                )?,
                enhanced_match_reason: row.enhanced_match_reason.map(Into::into),
                feedback: row.feedback,
            },
            created: row.created,
            updated: row.updated,
        })
    }
}
impl TryFrom<MatchRow> for SearchFilterMatchView {
    type Error = SearchFilterRowMappingError;
    fn try_from(row: MatchRow) -> Result<Self, Self::Error> {
        price_match_valuation(row.price_valuation_basis.as_deref(), row.price_fx_rate_id)?;
        Ok(Self {
            user_id: UserId::try_from(row.user_id)?,
            search_filter_id: UserSearchFilterId::try_from(row.user_search_filter_id)?,
            search_filter_name: row.user_search_filter_name.map(name).transpose()?,
            product_listing_id: ProductListingId::try_from(row.product_listing_id)?,
            origin_event_id: EventId::try_from(row.origin_event_id)?,
            enhanced_match_reason: row.enhanced_match_reason.map(Into::into),
            feedback: row.feedback,
            created: row.created,
            updated: row.updated,
        })
    }
}

fn price_match_valuation(
    basis: Option<&str>,
    fx_rate_id: Option<uuid::Uuid>,
) -> Result<Option<search_filter_core::PriceMatchValuation>, SearchFilterRowMappingError> {
    match (basis, fx_rate_id) {
        (None, None) => Ok(None),
        (Some(basis), Some(fx_rate_id)) => ProductListingPriceValuationBasis::iter()
            .find(|candidate| candidate.as_str() == basis)
            .map(|basis| {
                FxRateId::try_from(fx_rate_id)
                    .map(|fx_rate_id| search_filter_core::PriceMatchValuation { basis, fx_rate_id })
                    .map_err(SearchFilterRowMappingError::InvalidObjectId)
            })
            .ok_or(SearchFilterRowMappingError::InvalidPriceMatchValuation)?
            .map(Some),
        _ => Err(SearchFilterRowMappingError::InvalidPriceMatchValuation),
    }
}

pub(crate) fn state(value: &str) -> Result<SearchFilterState, SearchFilterRowMappingError> {
    SearchFilterState::from_code(value).ok_or(SearchFilterRowMappingError::InvalidState)
}
pub(crate) fn name(v: String) -> Result<UserSearchFilterName, SearchFilterRowMappingError> {
    if v.len() > 255 {
        Err(SearchFilterRowMappingError::NameTooLong)
    } else {
        Ok(v.into())
    }
}

fn serialize_code<T, S>(
    value: &T,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy,
    S: serde::Serializer,
{
    serializer.serialize_str(code(*value))
}

fn deserialize_code<'de, T, D>(deserializer: D, parse: fn(&str) -> Option<T>) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    parse(&value).ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
}

fn serialize_set_code<T, S>(
    values: &HashSet<T>,
    serializer: S,
    code: fn(T) -> &'static str,
) -> Result<S::Ok, S::Error>
where
    T: Copy + Eq + std::hash::Hash,
    S: serde::Serializer,
{
    serializer.collect_seq(values.iter().map(|value| code(*value)))
}

fn deserialize_set_code<'de, T, D>(
    deserializer: D,
    parse: fn(&str) -> Option<T>,
) -> Result<HashSet<T>, D::Error>
where
    T: Eq + std::hash::Hash,
    D: serde::Deserializer<'de>,
{
    Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(|value| {
            parse(&value)
                .ok_or_else(|| serde::de::Error::custom(format!("unsupported code `{value}`")))
        })
        .collect()
}

mod language {
    use super::*;

    pub(crate) fn serialize<S>(value: &Language, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, Language::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Language, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, Language::from_code)
    }
}

mod currency {
    use super::*;

    pub(crate) fn serialize<S>(value: &Currency, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, Currency::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Currency, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, Currency::from_code)
    }
}

mod canonical_uuid_set {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &HashSet<uuid::Uuid>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(values.iter().map(uuid::Uuid::to_string))
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<HashSet<uuid::Uuid>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Vec::<String>::deserialize(deserializer)?
            .into_iter()
            .map(|value| {
                let uuid = uuid::Uuid::parse_str(&value).map_err(serde::de::Error::custom)?;
                if uuid.to_string() != value {
                    return Err(serde::de::Error::custom(
                        "persisted UUID must use canonical hyphenated lowercase text",
                    ));
                }
                Ok(uuid)
            })
            .collect()
    }
}

mod listing_availability {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &HashSet<ListingAvailability>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_set_code(values, serializer, ListingAvailability::as_str)
    }

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashSet<ListingAvailability>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_set_code(deserializer, ListingAvailability::from_code)
    }
}

mod listing_orderability {
    use super::*;

    pub(crate) fn serialize<S>(
        values: &HashSet<ListingOrderability>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_set_code(values, serializer, ListingOrderability::as_str)
    }

    pub(crate) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<HashSet<ListingOrderability>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_set_code(deserializer, ListingOrderability::from_code)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductListingSearchJson {
    #[serde(with = "language")]
    language: Language,
    #[serde(with = "currency")]
    currency: Currency,
    product_listing_query: Vec<domain_primitives::query::text_query::TextQuery<1>>,
    enhanced_search_description: Option<String>,
    #[serde(with = "canonical_uuid_set")]
    exclude_product_listing_id_query: HashSet<uuid::Uuid>,
    #[serde(with = "canonical_uuid_set")]
    listing_source_id_query: HashSet<uuid::Uuid>,
    #[serde(with = "canonical_uuid_set")]
    exclude_listing_source_id_query: HashSet<uuid::Uuid>,
    price_query: Option<RangeQuery<u64>>,
    availability_query: Option<ListingAvailabilityQueryJson>,
    created_query: Option<TimeRangeJson>,
    updated_query: Option<TimeRangeJson>,
    lot_bidding_opens_query: Option<TimeRangeJson>,
    lot_scheduled_closes_query: Option<TimeRangeJson>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListingAvailabilityQueryJson {
    #[serde(with = "listing_availability")]
    availability: HashSet<ListingAvailability>,
    #[serde(with = "listing_orderability")]
    orderability: HashSet<ListingOrderability>,
    include_unspecified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeRangeJson {
    min: Option<String>,
    max: Option<String>,
}
impl TryFrom<RangeQuery<OffsetDateTime>> for TimeRangeJson {
    type Error = ProductListingSearchJsonMappingError;
    fn try_from(v: RangeQuery<OffsetDateTime>) -> Result<Self, Self::Error> {
        Ok(Self {
            min: v
                .min
                .map(|v| v.format(&Rfc3339))
                .transpose()
                .map_err(ProductListingSearchJsonMappingError::FormatTimestamp)?,
            max: v
                .max
                .map(|v| v.format(&Rfc3339))
                .transpose()
                .map_err(ProductListingSearchJsonMappingError::FormatTimestamp)?,
        })
    }
}
impl TryFrom<TimeRangeJson> for RangeQuery<OffsetDateTime> {
    type Error = ProductListingSearchJsonMappingError;
    fn try_from(v: TimeRangeJson) -> Result<Self, Self::Error> {
        Ok(Self {
            min: v
                .min
                .map(|v| OffsetDateTime::parse(&v, &Rfc3339))
                .transpose()
                .map_err(ProductListingSearchJsonMappingError::ParseTimestamp)?,
            max: v
                .max
                .map(|v| OffsetDateTime::parse(&v, &Rfc3339))
                .transpose()
                .map_err(ProductListingSearchJsonMappingError::ParseTimestamp)?,
        })
    }
}
impl TryFrom<&ProductListingSearch> for ProductListingSearchJson {
    type Error = ProductListingSearchJsonMappingError;

    fn try_from(v: &ProductListingSearch) -> Result<Self, Self::Error> {
        Ok(Self {
            language: v.language,
            currency: v.currency,
            product_listing_query: v.product_listing_query.clone(),
            enhanced_search_description: v
                .enhanced_search_description
                .as_ref()
                .map(ToString::to_string),
            exclude_product_listing_id_query: v
                .exclude_product_listing_id_query
                .iter()
                .copied()
                .map(ProductListingId::into_uuid)
                .collect(),
            listing_source_id_query: v
                .listing_source_id_query
                .iter()
                .copied()
                .map(|id| id.into_uuid())
                .collect(),
            exclude_listing_source_id_query: v
                .exclude_listing_source_id_query
                .iter()
                .copied()
                .map(|id| id.into_uuid())
                .collect(),
            price_query: v.price_query.map(|v| v.map(u64::from)),
            availability_query: v.availability_query.as_ref().map(|query| {
                ListingAvailabilityQueryJson {
                    availability: query.any_of.iter().copied().collect(),
                    orderability: query.orderability.iter().copied().collect(),
                    include_unspecified: query.include_unspecified,
                }
            }),
            created_query: v.created_query.map(TimeRangeJson::try_from).transpose()?,
            updated_query: v.updated_query.map(TimeRangeJson::try_from).transpose()?,
            lot_bidding_opens_query: v
                .lot_bidding_opens_query
                .map(TimeRangeJson::try_from)
                .transpose()?,
            lot_scheduled_closes_query: v
                .lot_scheduled_closes_query
                .map(TimeRangeJson::try_from)
                .transpose()?,
        })
    }
}
pub(crate) fn product_search_from_json(
    v: serde_json::Value,
) -> Result<ProductListingSearch, ProductListingSearchJsonMappingError> {
    let j: ProductListingSearchJson =
        serde_json::from_value(v).map_err(ProductListingSearchJsonMappingError::Deserialize)?;
    Ok(ProductListingSearch {
        language: j.language,
        currency: j.currency,
        product_listing_query: j.product_listing_query,
        enhanced_search_description: j
            .enhanced_search_description
            .map(EnhancedSearchDescription::try_from)
            .transpose()
            .map_err(ProductListingSearchJsonMappingError::EnhancedSearchDescription)?,
        exclude_product_listing_id_query: j
            .exclude_product_listing_id_query
            .into_iter()
            .map(ProductListingId::try_from)
            .collect::<Result<HashSet<_>, _>>()
            .map_err(ProductListingSearchJsonMappingError::ObjectId)?
            .into(),
        listing_source_id_query: j
            .listing_source_id_query
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<HashSet<_>, _>>()
            .map_err(ProductListingSearchJsonMappingError::ObjectId)?
            .into(),
        exclude_listing_source_id_query: j
            .exclude_listing_source_id_query
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<HashSet<_>, _>>()
            .map_err(ProductListingSearchJsonMappingError::ObjectId)?
            .into(),
        price_query: j.price_query.map(|v| v.map(Into::into)),
        availability_query: j.availability_query.map(|query| ListingAvailabilityQuery {
            any_of: query.availability.into(),
            orderability: query.orderability.into(),
            include_unspecified: query.include_unspecified,
        }),
        created_query: j.created_query.map(TryInto::try_into).transpose()?,
        updated_query: j.updated_query.map(TryInto::try_into).transpose()?,
        lot_bidding_opens_query: j
            .lot_bidding_opens_query
            .map(TryInto::try_into)
            .transpose()?,
        lot_scheduled_closes_query: j
            .lot_scheduled_closes_query
            .map(TryInto::try_into)
            .transpose()?,
    })
}
pub(crate) fn product_search_to_json(
    v: &ProductListingSearch,
) -> Result<serde_json::Value, ProductListingSearchJsonMappingError> {
    serde_json::to_value(ProductListingSearchJson::try_from(v)?)
        .map_err(ProductListingSearchJsonMappingError::Serialize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use money::Currency;

    fn filter_row(
        search_filter_id: uuid::Uuid,
        user_id: uuid::Uuid,
    ) -> Result<FilterRow, ProductListingSearchJsonMappingError> {
        Ok(FilterRow {
            user_search_filter_id: search_filter_id,
            user_id,
            name: "filter".to_owned(),
            notifications: true,
            state: SearchFilterState::Active.as_str().to_owned(),
            search: product_search_to_json(&ProductListingSearch::new(
                Language::En,
                Currency::Eur,
            ))?,
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        })
    }

    fn match_row() -> MatchRow {
        MatchRow {
            user_id: UserId::new().into_uuid(),
            user_search_filter_id: UserSearchFilterId::new().into_uuid(),
            product_listing_id: ProductListingId::new().into_uuid(),
            origin_event_id: EventId::new().into_uuid(),
            price_valuation_basis: None,
            price_fx_rate_id: None,
            user_search_filter_name: Some("filter".to_owned()),
            enhanced_match_reason: None,
            feedback: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn should_round_trip_v7_filter_row_ids() -> Result<(), Box<dyn Error>> {
        let search_filter_id = UserSearchFilterId::new();
        let user_id = UserId::new();

        let persisted =
            filter_row(search_filter_id.into_uuid(), user_id.into_uuid())?.into_persisted()?;
        let view = filter_row(search_filter_id.into_uuid(), user_id.into_uuid())?.into_view()?;
        let projection =
            filter_row(search_filter_id.into_uuid(), user_id.into_uuid())?.into_projection()?;

        assert_eq!(search_filter_id, persisted.filter.id());
        assert_eq!(user_id, persisted.filter.user_id());
        assert_eq!(search_filter_id, view.search_filter_id);
        assert_eq!(user_id, view.user_id);
        assert_eq!(search_filter_id, projection.view.search_filter_id);
        assert_eq!(user_id, projection.view.user_id);
        Ok(())
    }

    #[test]
    fn should_reject_v4_filter_row_ids() -> Result<(), Box<dyn Error>> {
        assert!(matches!(
            filter_row(uuid::Uuid::new_v4(), UserId::new().into_uuid())?.into_persisted(),
            Err(SearchFilterRepositoryError::InvalidPersistedState { .. })
        ));
        assert!(matches!(
            filter_row(UserSearchFilterId::new().into_uuid(), uuid::Uuid::new_v4())?.into_view(),
            Err(SearchFilterReadError::InvalidPersistedState)
        ));
        assert!(matches!(
            filter_row(uuid::Uuid::new_v4(), UserId::new().into_uuid())?.into_projection(),
            Err(SearchFilterIndexReadError::InvalidPersistedState { .. })
        ));
        Ok(())
    }

    #[test]
    fn should_round_trip_v7_match_row_ids() {
        let row = match_row();
        let expected = (
            row.user_id,
            row.user_search_filter_id,
            row.product_listing_id,
            row.origin_event_id,
        );
        let persisted = PersistedSearchFilterMatch::try_from(row)
            .unwrap_or_else(|error| panic!("valid match row failed: {error}"));
        let view = SearchFilterMatchView::try_from(match_row())
            .unwrap_or_else(|error| panic!("valid match view row failed: {error}"));

        assert_eq!(expected.0, persisted.product_match.user_id.into_uuid());
        assert_eq!(
            expected.1,
            persisted.product_match.user_search_filter_id.into_uuid()
        );
        assert_eq!(
            expected.2,
            persisted.product_match.product_listing_id.into_uuid()
        );
        assert_eq!(
            expected.3,
            persisted.product_match.origin_event_id.into_uuid()
        );
        assert_eq!(view.user_id.into_uuid().get_version_num(), 7);
        assert_eq!(view.search_filter_id.into_uuid().get_version_num(), 7);
        assert_eq!(view.product_listing_id.into_uuid().get_version_num(), 7);
        assert_eq!(view.origin_event_id.into_uuid().get_version_num(), 7);
    }

    #[test]
    fn should_reject_v4_match_row_ids() {
        for field in 0..5 {
            let mut row = match_row();
            match field {
                0 => row.user_id = uuid::Uuid::new_v4(),
                1 => row.user_search_filter_id = uuid::Uuid::new_v4(),
                2 => row.product_listing_id = uuid::Uuid::new_v4(),
                3 => row.origin_event_id = uuid::Uuid::new_v4(),
                _ => {
                    row.price_valuation_basis = Some(
                        ProductListingPriceValuationBasis::Current
                            .as_str()
                            .to_owned(),
                    );
                    row.price_fx_rate_id = Some(uuid::Uuid::new_v4());
                }
            }

            assert!(matches!(
                PersistedSearchFilterMatch::try_from(row),
                Err(SearchFilterRowMappingError::InvalidObjectId(
                    domain_primitives::object_id::ObjectIdError::UnsupportedUuidVersion {
                        actual: 4
                    }
                ))
            ));
        }
    }

    #[test]
    fn should_round_trip_full_product_search_json() {
        let search = ProductListingSearch::new(Language::De, Currency::Usd)
            .with_product_listing_query(match "vase".try_into() {
                Ok(v) => v,
                Err(e) => panic!("bad test value: {e}"),
            });
        let json = match product_search_to_json(&search) {
            Ok(v) => v,
            Err(_) => panic!("serialize"),
        };
        let decoded = match product_search_from_json(json) {
            Ok(search) => search,
            Err(error) => panic!("failed to deserialize product search: {error}"),
        };
        assert_eq!(search, decoded);
    }

    #[test]
    fn should_round_trip_object_id_filters_as_raw_uuid_json() -> Result<(), Box<dyn Error>> {
        let excluded_product_listing_id = ProductListingId::new();
        let included_listing_source_id = ListingSourceId::new();
        let excluded_listing_source_id = ListingSourceId::new();
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur)
            .with_listing_source_id_query(HashSet::from([included_listing_source_id]).into())
            .with_exclude_listing_source_id_query(
                HashSet::from([excluded_listing_source_id]).into(),
            );
        search.exclude_product_listing_id_query =
            HashSet::from([excluded_product_listing_id]).into();

        let persisted = product_search_to_json(&search)?;

        assert_eq!(
            Some(&serde_json::json!(
                excluded_product_listing_id.into_uuid().to_string()
            )),
            persisted.pointer("/exclude_product_listing_id_query/0")
        );
        assert_eq!(
            Some(&serde_json::json!(
                included_listing_source_id.into_uuid().to_string()
            )),
            persisted.pointer("/listing_source_id_query/0")
        );
        assert_eq!(
            Some(&serde_json::json!(
                excluded_listing_source_id.into_uuid().to_string()
            )),
            persisted.pointer("/exclude_listing_source_id_query/0")
        );
        assert_eq!(search, product_search_from_json(persisted)?);
        Ok(())
    }

    #[test]
    fn should_reject_noncanonical_ids_in_persisted_product_search_json()
    -> Result<(), Box<dyn Error>> {
        let id = uuid::Uuid::parse_str("01890f3e-3b7c-7cc2-98c8-5f8b8a5d5f0d")?;
        let noncanonical_values = [
            id.simple().to_string(),
            id.braced().to_string(),
            id.urn().to_string(),
            id.to_string().to_uppercase(),
        ];

        for field in [
            "exclude_product_listing_id_query",
            "listing_source_id_query",
            "exclude_listing_source_id_query",
        ] {
            for value in &noncanonical_values {
                let mut persisted = product_search_to_json(&ProductListingSearch::new(
                    Language::En,
                    Currency::Eur,
                ))?;
                persisted[field] = serde_json::json!([value]);

                let error = product_search_from_json(persisted);
                assert!(matches!(
                    error,
                    Err(ProductListingSearchJsonMappingError::Deserialize(source))
                        if source.to_string().contains(
                            "persisted UUID must use canonical hyphenated lowercase text"
                        )
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn should_reject_v4_ids_in_persisted_product_search_json() -> Result<(), Box<dyn Error>> {
        for field in [
            "exclude_product_listing_id_query",
            "listing_source_id_query",
            "exclude_listing_source_id_query",
        ] {
            let mut persisted =
                product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur))?;
            persisted[field] = serde_json::json!([uuid::Uuid::new_v4().to_string()]);

            assert!(matches!(
                product_search_from_json(persisted),
                Err(ProductListingSearchJsonMappingError::ObjectId(
                    domain_primitives::object_id::ObjectIdError::UnsupportedUuidVersion {
                        actual: 4
                    }
                ))
            ));
        }
        Ok(())
    }

    #[test]
    fn should_preserve_set_semantics_for_persisted_product_search_ids() -> Result<(), Box<dyn Error>>
    {
        let product_listing_id = ProductListingId::new();
        let listing_source_id = ListingSourceId::new();
        let product_listing_uuid = product_listing_id.into_uuid().to_string();
        let listing_source_uuid = listing_source_id.into_uuid().to_string();
        let mut persisted =
            product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur))?;
        persisted["exclude_product_listing_id_query"] =
            serde_json::json!([product_listing_uuid, product_listing_uuid]);
        persisted["listing_source_id_query"] =
            serde_json::json!([listing_source_uuid, listing_source_uuid]);
        persisted["exclude_listing_source_id_query"] =
            serde_json::json!([listing_source_uuid, listing_source_uuid]);

        let decoded = product_search_from_json(persisted)?;

        assert_eq!(1, decoded.exclude_product_listing_id_query.len());
        assert!(
            decoded
                .exclude_product_listing_id_query
                .contains(&product_listing_id)
        );
        assert_eq!(1, decoded.listing_source_id_query.len());
        assert!(decoded.listing_source_id_query.contains(&listing_source_id));
        assert_eq!(1, decoded.exclude_listing_source_id_query.len());
        assert!(
            decoded
                .exclude_listing_source_id_query
                .contains(&listing_source_id)
        );
        Ok(())
    }

    #[test]
    fn should_preserve_canonical_availability_query_codes() -> Result<(), Box<dyn Error>> {
        let mut persisted =
            product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur))?;
        persisted["availability_query"] = serde_json::json!({
            "availability": ["IN_STOCK"],
            "orderability": ["ORDERABLE_NOW"],
            "include_unspecified": true
        });

        assert_eq!(Some(&serde_json::json!("en")), persisted.get("language"));
        assert_eq!(Some(&serde_json::json!("EUR")), persisted.get("currency"));
        assert_eq!(
            Some(&serde_json::json!("IN_STOCK")),
            persisted.pointer("/availability_query/availability/0")
        );
        assert_eq!(
            Some(&serde_json::json!("ORDERABLE_NOW")),
            persisted.pointer("/availability_query/orderability/0")
        );
        assert_eq!(
            Some(&serde_json::json!(true)),
            persisted.pointer("/availability_query/include_unspecified")
        );

        let decoded = product_search_from_json(persisted)?;
        assert_eq!(Language::En, decoded.language);
        assert_eq!(Currency::Eur, decoded.currency);
        assert_eq!(
            Some(ListingAvailabilityQuery {
                any_of: HashSet::from([ListingAvailability::InStock]).into(),
                orderability: HashSet::from([ListingOrderability::OrderableNow]).into(),
                include_unspecified: true,
            }),
            decoded.availability_query
        );
        Ok(())
    }
    #[test]
    fn should_preserve_absent_and_configured_empty_availability_queries()
    -> Result<(), Box<dyn Error>> {
        let absent =
            product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur))?;
        assert_eq!(
            Some(&serde_json::Value::Null),
            absent.get("availability_query")
        );

        let mut configured_empty = ProductListingSearch::new(Language::En, Currency::Eur);
        configured_empty.availability_query = Some(ListingAvailabilityQuery {
            any_of: Default::default(),
            orderability: Default::default(),
            include_unspecified: false,
        });
        let configured_empty = product_search_to_json(&configured_empty)?;
        assert_eq!(
            Some(&serde_json::json!({
                "availability": [],
                "orderability": [],
                "include_unspecified": false
            })),
            configured_empty.get("availability_query")
        );
        Ok(())
    }

    #[test]
    fn should_serialize_every_product_search_field() {
        let json =
            match product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur)) {
                Ok(value) => value,
                Err(_) => panic!("failed to serialize product search"),
            };
        let object = match json.as_object() {
            Some(value) => value,
            None => panic!("product search JSON must be an object"),
        };

        assert_eq!(13, object.len());
        assert!(object.contains_key("lot_scheduled_closes_query"));
        assert!(object.contains_key("listing_source_id_query"));
        assert!(object.contains_key("exclude_listing_source_id_query"));
        assert!(
            !object.contains_key("obsolete_query"),
            "unknown keys must not persist"
        );
    }

    #[test]
    fn should_reject_obsolete_product_search_filter_keys() -> Result<(), Box<dyn Error>> {
        let mut persisted =
            product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur))?;
        persisted["obsolete_query"] = serde_json::json!([]);

        assert!(matches!(
            product_search_from_json(persisted),
            Err(ProductListingSearchJsonMappingError::Deserialize(_))
        ));
        Ok(())
    }

    #[test]
    fn should_preserve_incomplete_product_search_json_source() {
        let error = match product_search_from_json(serde_json::json!({})) {
            Ok(_) => panic!("incomplete product search JSON must fail"),
            Err(error) => error,
        };

        assert!(std::error::Error::source(&error).is_some());
        assert!(matches!(
            error,
            ProductListingSearchJsonMappingError::Deserialize(_)
        ));
    }

    #[test]
    fn should_parse_each_canonical_state() {
        for expected in SearchFilterState::iter() {
            assert!(matches!(state(expected.as_str()), Ok(actual) if actual == expected));
        }
    }

    #[test]
    fn should_parse_each_canonical_price_match_valuation_basis() {
        let expected_fx_rate_id = FxRateId::new();
        let fx_rate_id = expected_fx_rate_id.into_uuid();

        for expected in ProductListingPriceValuationBasis::iter() {
            let valuation = price_match_valuation(Some(expected.as_str()), Some(fx_rate_id));

            assert!(matches!(
                valuation,
                Ok(Some(actual)) if actual.basis == expected && actual.fx_rate_id == expected_fx_rate_id
            ));
        }
    }

    #[test]
    fn should_reject_v4_price_match_fx_rate_id() {
        assert!(matches!(
            price_match_valuation(
                Some(ProductListingPriceValuationBasis::Current.as_str()),
                Some(uuid::Uuid::new_v4())
            ),
            Err(SearchFilterRowMappingError::InvalidObjectId(
                domain_primitives::object_id::ObjectIdError::UnsupportedUuidVersion { actual: 4 }
            ))
        ));
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_price_match_valuation_bases() {
        let fx_rate_id = FxRateId::new().into_uuid();

        for basis in ["bad", "current"] {
            assert!(matches!(
                price_match_valuation(Some(basis), Some(fx_rate_id)),
                Err(SearchFilterRowMappingError::InvalidPriceMatchValuation)
            ));
        }
    }

    #[test]
    fn should_reject_unknown_and_noncanonical_states() {
        for value in ["bad", "active"] {
            assert!(matches!(
                state(value),
                Err(SearchFilterRowMappingError::InvalidState)
            ));
        }
    }

    #[test]
    fn should_preserve_invalid_filter_row_mapping_source() {
        let search =
            match product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur)) {
                Ok(search) => search,
                Err(error) => panic!("failed to create product search JSON: {error}"),
            };
        let error = match (FilterRow {
            user_search_filter_id: UserSearchFilterId::new().into_uuid(),
            user_id: UserId::new().into_uuid(),
            name: "x".repeat(256),
            notifications: true,
            state: "ACTIVE".to_owned(),
            search,
            embedding: None,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
            version: 1,
        })
        .into_persisted()
        {
            Ok(_) => panic!("overlong persisted filter name must fail"),
            Err(error) => error,
        };

        let SearchFilterRepositoryError::InvalidPersistedState { source } = error else {
            panic!("expected invalid persisted search-filter state");
        };
        assert!(matches!(
            source.downcast_ref::<SearchFilterRowMappingError>(),
            Some(SearchFilterRowMappingError::NameTooLong)
        ));
    }

    #[test]
    fn should_reject_unknown_product_search_field() {
        let mut json =
            match product_search_to_json(&ProductListingSearch::new(Language::En, Currency::Eur)) {
                Ok(value) => value,
                Err(_) => panic!("failed to serialize product search"),
            };
        let object = match json.as_object_mut() {
            Some(value) => value,
            None => panic!("product search JSON must be an object"),
        };
        object.insert("unexpected".into(), serde_json::Value::Null);

        assert!(product_search_from_json(json).is_err());
    }
}

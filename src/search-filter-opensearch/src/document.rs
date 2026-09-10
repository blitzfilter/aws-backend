use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;

use listing_source_core::ListingSourceId;
use localization::Language;
use money::{Currency, MonetaryAmount};
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::listing_orderability::ListingOrderability;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, EnhancedSearchDescriptionError, ListingAvailabilityQuery,
    ProductListingSearch,
};
use product_listing_opensearch::build_percolator_query;
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use search_filter_service::ports::{SearchFilterProjection, SearchFilterView};
use serde::ser::Error as _;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use user_core::user_id::UserId;

const PRODUCT_SEARCH_FIELDS: [&str; 13] = [
    "language",
    "currency",
    "productQuery",
    "enhancedSearchDescription",
    "excludeProductId",
    "listingSourceId",
    "excludeListingSourceId",
    "price",
    "availability",
    "created",
    "updated",
    "auctionStart",
    "auctionEnd",
];

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

mod search_filter_state {
    use super::*;

    pub(crate) fn serialize<S>(value: &SearchFilterState, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serialize_code(value, serializer, SearchFilterState::as_str)
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<SearchFilterState, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_code(deserializer, SearchFilterState::from_code)
    }
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
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchFilterDocument {
    pub user_search_filter_id: UserSearchFilterId,
    pub user_id: UserId,
    pub name: UserSearchFilterName,
    pub notifications: bool,
    #[serde(with = "search_filter_state")]
    pub state: SearchFilterState,
    pub source_version: i64,
    pub search: serde_json::Value,
    pub query: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub embedding: Option<Vec<f32>>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

/// Decode failure for the complete product-search payload stored in a search document.
#[derive(Debug, thiserror::Error)]
pub enum ProductListingSearchDocumentMappingError {
    #[error("OpenSearch document has malformed product search JSON")]
    Deserialize {
        #[source]
        source: serde_json::Error,
    },
    #[error("OpenSearch document has an invalid product search timestamp")]
    InvalidTimestamp,
    #[error("OpenSearch document has an invalid enhanced search description")]
    InvalidEnhancedSearchDescription {
        #[source]
        source: EnhancedSearchDescriptionError,
    },
}

impl TryFrom<&SearchFilterProjection> for SearchFilterDocument {
    type Error = serde_json::Error;

    fn try_from(projection: &SearchFilterProjection) -> Result<Self, Self::Error> {
        let view = &projection.view;
        Ok(Self {
            user_search_filter_id: view.search_filter_id,
            user_id: view.user_id,
            name: view.name.clone(),
            notifications: view.notifications,
            state: view.state,
            source_version: projection.source_version,
            search: product_search_to_value(&view.search)?,
            query: build_percolator_query(&view.search)?,
            embedding: view.embedding.clone(),
            created: view.created,
            updated: view.updated,
        })
    }
}

impl TryFrom<SearchFilterDocument> for SearchFilterView {
    type Error = ProductListingSearchDocumentMappingError;

    fn try_from(document: SearchFilterDocument) -> Result<Self, Self::Error> {
        Ok(SearchFilterView {
            search_filter_id: document.user_search_filter_id,
            user_id: document.user_id,
            name: document.name,
            notifications: document.notifications,
            state: document.state,
            search: product_search_from_value(document.search)?,
            embedding: document.embedding,
            created: document.created,
            updated: document.updated,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProductListingSearchDocument {
    #[serde(with = "language")]
    language: Language,
    #[serde(with = "currency")]
    currency: Currency,
    #[serde(rename = "productQuery")]
    product_listing_query: Vec<TextQuery<1>>,
    #[serde(rename = "enhancedSearchDescription")]
    enhanced_search_description: Option<String>,
    #[serde(rename = "excludeProductId")]
    exclude_product_listing_id_query: HashSet<ProductListingId>,
    #[serde(rename = "listingSourceId")]
    listing_source_id_query: HashSet<ListingSourceId>,
    #[serde(rename = "excludeListingSourceId")]
    exclude_listing_source_id_query: HashSet<ListingSourceId>,
    #[serde(rename = "price")]
    price_query: Option<RangeQuery<u64>>,
    #[serde(rename = "availability")]
    availability_query: Option<ListingAvailabilityQueryDocument>,
    #[serde(rename = "created")]
    created_query: Option<TimeRangeDocument>,
    #[serde(rename = "updated")]
    updated_query: Option<TimeRangeDocument>,
    #[serde(rename = "auctionStart")]
    auction_start_query: Option<TimeRangeDocument>,
    #[serde(rename = "auctionEnd")]
    auction_end_query: Option<TimeRangeDocument>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListingAvailabilityQueryDocument {
    #[serde(with = "listing_availability")]
    availability: HashSet<ListingAvailability>,
    #[serde(with = "listing_orderability")]
    orderability: HashSet<ListingOrderability>,
    include_unspecified: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimeRangeDocument {
    min: Option<String>,
    max: Option<String>,
}

impl TryFrom<RangeQuery<OffsetDateTime>> for TimeRangeDocument {
    type Error = serde_json::Error;

    fn try_from(value: RangeQuery<OffsetDateTime>) -> Result<Self, Self::Error> {
        Ok(Self {
            min: value
                .min
                .map(|time| time.format(&Rfc3339))
                .transpose()
                .map_err(serde_json::Error::custom)?,
            max: value
                .max
                .map(|time| time.format(&Rfc3339))
                .transpose()
                .map_err(serde_json::Error::custom)?,
        })
    }
}

impl TryFrom<TimeRangeDocument> for RangeQuery<OffsetDateTime> {
    type Error = ();

    fn try_from(value: TimeRangeDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            min: value
                .min
                .map(|time| OffsetDateTime::parse(&time, &Rfc3339))
                .transpose()
                .map_err(|_| ())?,
            max: value
                .max
                .map(|time| OffsetDateTime::parse(&time, &Rfc3339))
                .transpose()
                .map_err(|_| ())?,
        })
    }
}

impl TryFrom<&ProductListingSearch> for ProductListingSearchDocument {
    type Error = serde_json::Error;

    fn try_from(search: &ProductListingSearch) -> Result<Self, Self::Error> {
        Ok(Self {
            language: search.language,
            currency: search.currency,
            product_listing_query: search.product_listing_query.clone(),
            enhanced_search_description: search
                .enhanced_search_description
                .as_ref()
                .map(ToString::to_string),
            exclude_product_listing_id_query: search
                .exclude_product_listing_id_query
                .iter()
                .copied()
                .collect(),
            listing_source_id_query: search.listing_source_id_query.iter().copied().collect(),
            exclude_listing_source_id_query: search
                .exclude_listing_source_id_query
                .iter()
                .copied()
                .collect(),
            price_query: search.price_query.map(|range| range.map(u64::from)),
            availability_query: search.availability_query.as_ref().map(|query| {
                ListingAvailabilityQueryDocument {
                    availability: query.any_of.iter().copied().collect(),
                    orderability: query.orderability.iter().copied().collect(),
                    include_unspecified: query.include_unspecified,
                }
            }),
            created_query: search.created_query.map(TryInto::try_into).transpose()?,
            updated_query: search.updated_query.map(TryInto::try_into).transpose()?,
            auction_start_query: search
                .auction_start_query
                .map(TryInto::try_into)
                .transpose()?,
            auction_end_query: search
                .auction_end_query
                .map(TryInto::try_into)
                .transpose()?,
        })
    }
}

impl TryFrom<ProductListingSearchDocument> for ProductListingSearch {
    type Error = ProductListingSearchDocumentMappingError;

    fn try_from(document: ProductListingSearchDocument) -> Result<Self, Self::Error> {
        Ok(Self {
            language: document.language,
            currency: document.currency,
            product_listing_query: document.product_listing_query,
            enhanced_search_description: document
                .enhanced_search_description
                .map(EnhancedSearchDescription::try_from)
                .transpose()
                .map_err(|source| {
                    ProductListingSearchDocumentMappingError::InvalidEnhancedSearchDescription {
                        source,
                    }
                })?,
            exclude_product_listing_id_query: document.exclude_product_listing_id_query.into(),
            listing_source_id_query: document.listing_source_id_query.into(),
            exclude_listing_source_id_query: document.exclude_listing_source_id_query.into(),
            price_query: document
                .price_query
                .map(|range| range.map(MonetaryAmount::from)),
            availability_query: document
                .availability_query
                .map(|query| ListingAvailabilityQuery {
                    any_of: query.availability.into(),
                    orderability: query.orderability.into(),
                    include_unspecified: query.include_unspecified,
                }),
            created_query: document.created_query.map(parse_time_range).transpose()?,
            updated_query: document.updated_query.map(parse_time_range).transpose()?,
            auction_start_query: document
                .auction_start_query
                .map(parse_time_range)
                .transpose()?,
            auction_end_query: document
                .auction_end_query
                .map(parse_time_range)
                .transpose()?,
        })
    }
}

fn parse_time_range(
    value: TimeRangeDocument,
) -> Result<RangeQuery<OffsetDateTime>, ProductListingSearchDocumentMappingError> {
    value
        .try_into()
        .map_err(|_| ProductListingSearchDocumentMappingError::InvalidTimestamp)
}

fn product_search_to_value(
    search: &ProductListingSearch,
) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::to_value(ProductListingSearchDocument::try_from(search)?)
}

fn product_search_from_value(
    value: serde_json::Value,
) -> Result<ProductListingSearch, ProductListingSearchDocumentMappingError> {
    let Some(object) = value.as_object() else {
        return Err(ProductListingSearchDocumentMappingError::InvalidTimestamp);
    };
    if !PRODUCT_SEARCH_FIELDS
        .iter()
        .all(|field| object.contains_key(*field))
    {
        return Err(ProductListingSearchDocumentMappingError::InvalidTimestamp);
    }

    let document = serde_json::from_value::<ProductListingSearchDocument>(value)
        .map_err(|source| ProductListingSearchDocumentMappingError::Deserialize { source })?;
    document.try_into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::query::range_query::RangeQuery;
    use domain_primitives::query::text_query::TextQuery;
    use listing_source_core::ListingSourceId;
    use localization::Language;
    use money::Currency;
    use product_listing_core::product_listing_search::ListingAvailabilityQuery;
    use search_filter_service::ports::SearchFilterProjection;
    use std::collections::BTreeSet;
    use strum::IntoEnumIterator;
    use time::macros::datetime;

    fn projection(search: ProductListingSearch) -> SearchFilterProjection {
        SearchFilterProjection {
            view: SearchFilterView {
                search_filter_id: UserSearchFilterId::new(),
                user_id: UserId::new(),
                name: UserSearchFilterName::from("daily"),
                notifications: true,
                state: search_filter_core::search_filter_state::SearchFilterState::Active,
                search,
                embedding: Some(vec![1.0]),
                created: datetime!(2026-01-01 00:00:00 UTC),
                updated: datetime!(2026-01-02 00:00:00 UTC),
            },
            source_version: 12,
        }
    }

    #[test]
    fn should_store_authoritative_search_version_and_original_price_range()
    -> Result<(), Box<dyn std::error::Error>> {
        let search =
            ProductListingSearch::new(Language::En, Currency::Usd).with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(10_000_u64)),
                max: Some(MonetaryAmount::from(50_000_u64)),
            });
        let document = SearchFilterDocument::try_from(&projection(search))?;
        let value = serde_json::to_value(&document)?;

        assert_eq!(12, document.source_version);
        assert_eq!(
            Some(&serde_json::json!({ "gte": 10_000, "lte": 50_000 })),
            document
                .query
                .pointer("/bool/filter/0/range/priceByCurrency.usd")
        );
        assert!(value.get("compiledFxRateId").is_none());
        assert!(value.get("compiledFxGeneration").is_none());
        Ok(())
    }

    #[test]
    fn should_round_trip_search_filter_without_a_price_range()
    -> Result<(), Box<dyn std::error::Error>> {
        let excluded_product_listing_id = ProductListingId::new();
        let listing_source_id = ListingSourceId::new();
        let excluded_listing_source_id = ListingSourceId::new();
        let expected = projection(
            ProductListingSearch::new(Language::En, Currency::Usd)
                .with_exclude_product_listing_id_query(
                    std::collections::HashSet::from([excluded_product_listing_id]).into(),
                )
                .with_listing_source_id_query(
                    std::collections::HashSet::from([listing_source_id]).into(),
                )
                .with_exclude_listing_source_id_query(
                    std::collections::HashSet::from([excluded_listing_source_id]).into(),
                )
                .with_availability_query(ListingAvailabilityQuery {
                    any_of: std::collections::HashSet::from([ListingAvailability::InStock]).into(),
                    orderability: std::collections::HashSet::from([
                        ListingOrderability::OrderableNow,
                    ])
                    .into(),
                    include_unspecified: true,
                }),
        );
        let document = SearchFilterDocument::try_from(&expected)?;
        let value = serde_json::to_value(&document)?;

        assert!(!document.query.to_string().contains("priceByCurrency"));
        assert_eq!(
            Some(&serde_json::json!("en")),
            value.pointer("/search/language")
        );
        assert_eq!(
            Some(&serde_json::json!("USD")),
            value.pointer("/search/currency")
        );
        assert_eq!(
            Some(&serde_json::json!(
                expected.view.search_filter_id.to_string()
            )),
            value.get("userSearchFilterId")
        );
        assert_eq!(
            Some(&serde_json::json!(expected.view.user_id.to_string())),
            value.get("userId")
        );
        assert_eq!(
            Some(&serde_json::json!(excluded_product_listing_id.to_string())),
            value.pointer("/search/excludeProductId/0")
        );
        assert_eq!(
            Some(&serde_json::json!(listing_source_id.to_string())),
            value.pointer("/search/listingSourceId/0")
        );
        assert_eq!(
            Some(&serde_json::json!(excluded_listing_source_id.to_string())),
            value.pointer("/search/excludeListingSourceId/0")
        );
        assert!(value.pointer("/search/shopName").is_none());
        assert!(value.pointer("/search/sellerName").is_none());
        assert!(value.pointer("/search/shopType").is_none());
        assert!(value.pointer("/search/country").is_none());
        assert!(value.pointer("/search/continent").is_none());

        assert_eq!(
            Some(&serde_json::json!(listing_source_id.to_string())),
            value.pointer("/query/bool/filter/0/terms/listingSourceId/0")
        );
        assert_eq!(
            Some(&serde_json::json!(excluded_product_listing_id.to_string())),
            value.pointer("/query/bool/must_not/0/terms/productListingId/0")
        );
        assert_eq!(
            Some(&serde_json::json!(excluded_listing_source_id.to_string())),
            value.pointer("/query/bool/must_not/1/terms/listingSourceId/0")
        );
        assert_eq!(
            Some(&serde_json::json!("IN_STOCK")),
            value.pointer("/search/availability/availability/0")
        );
        assert_eq!(
            Some(&serde_json::json!("ORDERABLE_NOW")),
            value.pointer("/search/availability/orderability/0")
        );
        assert_eq!(
            Some(&serde_json::json!(true)),
            value.pointer("/search/availability/includeUnspecified")
        );
        assert_eq!(expected.view, SearchFilterView::try_from(document)?);
        Ok(())
    }

    #[test]
    fn should_reject_wrong_prefixes_for_typed_document_ids_and_sets()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = typed_id_document_value()?;
        let cases = [
            ("/userSearchFilterId", UserId::new().to_string()),
            ("/userId", UserSearchFilterId::new().to_string()),
            (
                "/search/excludeProductId/0",
                ListingSourceId::new().to_string(),
            ),
            (
                "/search/listingSourceId/0",
                ProductListingId::new().to_string(),
            ),
            (
                "/search/excludeListingSourceId/0",
                ProductListingId::new().to_string(),
            ),
        ];

        for (pointer, wrong_id) in cases {
            let mut malformed = value.clone();
            *malformed
                .pointer_mut(pointer)
                .ok_or_else(|| format!("missing test field `{pointer}`"))? =
                serde_json::json!(wrong_id);
            assert_search_filter_document_rejected(malformed);
        }
        Ok(())
    }

    #[test]
    fn should_reject_bare_uuids_for_typed_document_ids_and_sets()
    -> Result<(), Box<dyn std::error::Error>> {
        let projection = typed_id_projection();
        let value = serde_json::to_value(SearchFilterDocument::try_from(&projection)?)?;
        let search = &projection.view.search;
        let product_listing_id = search
            .exclude_product_listing_id_query
            .iter()
            .next()
            .ok_or("excluded ProductListing ID missing")?;
        let listing_source_id = search
            .listing_source_id_query
            .iter()
            .next()
            .ok_or("ListingSource ID missing")?;
        let excluded_listing_source_id = search
            .exclude_listing_source_id_query
            .iter()
            .next()
            .ok_or("excluded ListingSource ID missing")?;
        let cases = [
            (
                "/userSearchFilterId",
                projection.view.search_filter_id.as_uuid().to_string(),
            ),
            ("/userId", projection.view.user_id.as_uuid().to_string()),
            (
                "/search/excludeProductId/0",
                product_listing_id.as_uuid().to_string(),
            ),
            (
                "/search/listingSourceId/0",
                listing_source_id.as_uuid().to_string(),
            ),
            (
                "/search/excludeListingSourceId/0",
                excluded_listing_source_id.as_uuid().to_string(),
            ),
        ];

        for (pointer, bare_id) in cases {
            let mut malformed = value.clone();
            *malformed
                .pointer_mut(pointer)
                .ok_or_else(|| format!("missing test field `{pointer}`"))? =
                serde_json::json!(bare_id);
            assert_search_filter_document_rejected(malformed);
        }
        Ok(())
    }

    fn typed_id_projection() -> SearchFilterProjection {
        projection(
            ProductListingSearch::new(Language::En, Currency::Eur)
                .with_exclude_product_listing_id_query(
                    std::collections::HashSet::from([ProductListingId::new()]).into(),
                )
                .with_listing_source_id_query(
                    std::collections::HashSet::from([ListingSourceId::new()]).into(),
                )
                .with_exclude_listing_source_id_query(
                    std::collections::HashSet::from([ListingSourceId::new()]).into(),
                ),
        )
    }

    fn typed_id_document_value() -> Result<serde_json::Value, serde_json::Error> {
        SearchFilterDocument::try_from(&typed_id_projection()).and_then(serde_json::to_value)
    }

    fn assert_search_filter_document_rejected(value: serde_json::Value) {
        let result = serde_json::from_value::<SearchFilterDocument>(value)
            .map_err(|error| error.to_string())
            .and_then(|document| {
                SearchFilterView::try_from(document).map_err(|error| error.to_string())
            });
        assert!(result.is_err(), "malformed typed ID document was accepted");
    }

    #[test]
    fn should_keep_saved_filter_documents_and_percolator_fields_covered_by_mapping()
    -> Result<(), Box<dyn std::error::Error>> {
        let mapping: serde_json::Value = serde_json::from_str(include_str!(
            "../../../opensearch/mappings/user_search_filters.json"
        ))?;
        let representative = representative_search(Language::En, Currency::Usd)?;
        let document = SearchFilterDocument::try_from(&projection(representative.clone()))?;
        let document_value = serde_json::to_value(document)?;

        let document_fields = document_field_paths(&document_value, "");
        for field in &document_fields {
            let Some(field_mapping) = mapping_field(&mapping, field) else {
                return Err(format!(
                    "saved-filter document field `{field}` is missing from mapping"
                )
                .into());
            };
            let value = document_value
                .pointer(&format!("/{}", field.replace('.', "/")))
                .ok_or_else(|| {
                    format!("document field `{field}` disappeared while checking mapping")
                })?;
            if !mapping_accepts_value(field_mapping, value) {
                return Err(format!(
                    "mapping for saved-filter document field `{field}` rejects {value}"
                )
                .into());
            }
        }

        let mut percolator_fields = BTreeSet::new();
        for language in Language::iter() {
            collect_query_field_paths(
                &build_percolator_query(&representative_search(language, Currency::Eur)?)?,
                &mut percolator_fields,
            );
        }
        for currency in Currency::iter() {
            collect_query_field_paths(
                &build_percolator_query(&representative_search(Language::En, currency)?)?,
                &mut percolator_fields,
            );
        }

        for field in &percolator_fields {
            if mapping_field(&mapping, field).is_none() {
                return Err(
                    format!("percolator query field `{field}` is missing from mapping").into(),
                );
            }
        }

        assert_eq!(
            Some(&serde_json::json!("percolator")),
            mapping_field(&mapping, "query").and_then(|field| field.get("type"))
        );
        assert_eq!(
            Some(&serde_json::json!(768)),
            mapping_field(&mapping, "embedding").and_then(|field| field.get("dimension"))
        );
        Ok(())
    }

    fn representative_search(
        language: Language,
        currency: Currency,
    ) -> Result<ProductListingSearch, Box<dyn std::error::Error>> {
        let product_listing_query = TextQuery::<1>::try_from("renaissance cabinet")?;
        let enhanced_search_description =
            EnhancedSearchDescription::try_from("renaissance furniture")?;
        Ok(ProductListingSearch::new(language, currency)
            .with_product_listing_query(product_listing_query)
            .with_enhanced_search_description(enhanced_search_description)
            .with_exclude_product_listing_id_query(
                std::collections::HashSet::from([ProductListingId::new()]).into(),
            )
            .with_listing_source_id_query(
                std::collections::HashSet::from([ListingSourceId::new()]).into(),
            )
            .with_exclude_listing_source_id_query(
                std::collections::HashSet::from([ListingSourceId::new()]).into(),
            )
            .with_price_query(RangeQuery {
                min: Some(MonetaryAmount::from(10_000_u64)),
                max: Some(MonetaryAmount::from(50_000_u64)),
            })
            .with_availability_query(ListingAvailabilityQuery {
                any_of: std::collections::HashSet::from([ListingAvailability::InStock]).into(),
                orderability: std::collections::HashSet::from([ListingOrderability::OrderableNow])
                    .into(),
                include_unspecified: true,
            })
            .with_created_query(RangeQuery {
                min: Some(datetime!(2026-01-01 00:00:00 UTC)),
                max: Some(datetime!(2026-01-02 00:00:00 UTC)),
            })
            .with_updated_query(RangeQuery {
                min: Some(datetime!(2026-01-03 00:00:00 UTC)),
                max: Some(datetime!(2026-01-04 00:00:00 UTC)),
            })
            .with_auction_start_query(RangeQuery {
                min: Some(datetime!(2026-01-05 00:00:00 UTC)),
                max: Some(datetime!(2026-01-06 00:00:00 UTC)),
            })
            .with_auction_end_query(RangeQuery {
                min: Some(datetime!(2026-01-07 00:00:00 UTC)),
                max: Some(datetime!(2026-01-08 00:00:00 UTC)),
            }))
    }

    fn document_field_paths(value: &serde_json::Value, prefix: &str) -> BTreeSet<String> {
        let mut fields = BTreeSet::new();
        collect_document_field_paths(value, prefix, &mut fields);
        fields
    }

    fn collect_document_field_paths(
        value: &serde_json::Value,
        prefix: &str,
        fields: &mut BTreeSet<String>,
    ) {
        match value {
            serde_json::Value::Object(object) => {
                for (key, value) in object {
                    if prefix.is_empty() && key == "query" {
                        continue;
                    }
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    collect_document_field_paths(value, &path, fields);
                }
            }
            serde_json::Value::Array(values) if values.is_empty() => {}
            serde_json::Value::Array(_)
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {
                fields.insert(prefix.to_owned());
            }
            serde_json::Value::Null => {}
        }
    }

    fn mapping_field<'a>(
        mapping: &'a serde_json::Value,
        path: &str,
    ) -> Option<&'a serde_json::Value> {
        let mut properties = mapping.pointer("/mappings/properties")?;
        let mut segments = path.split('.').peekable();
        while let Some(segment) = segments.next() {
            let field = properties.get(segment)?;
            if segments.peek().is_none() {
                return Some(field);
            }
            properties = field.get("properties")?;
        }
        None
    }

    fn mapping_accepts_value(mapping: &serde_json::Value, value: &serde_json::Value) -> bool {
        if let serde_json::Value::Array(values) = value {
            return matches!(
                mapping.get("type").and_then(serde_json::Value::as_str),
                Some("knn_vector")
            ) || values
                .iter()
                .all(|value| mapping_accepts_value(mapping, value));
        }

        match mapping.get("type").and_then(serde_json::Value::as_str) {
            Some("keyword" | "text" | "date") => value.is_string(),
            Some("boolean") => value.is_boolean(),
            Some("long" | "unsigned_long") => value.is_number(),
            Some("percolator") => value.is_object(),
            _ => false,
        }
    }

    fn collect_query_field_paths(query: &serde_json::Value, fields: &mut BTreeSet<String>) {
        match query {
            serde_json::Value::Object(object) => {
                for operator in ["terms", "range", "match", "match_phrase"] {
                    if let Some(clauses) =
                        object.get(operator).and_then(serde_json::Value::as_object)
                    {
                        fields.extend(clauses.keys().cloned());
                    }
                }
                if let Some(field) = object
                    .get("exists")
                    .and_then(|exists| exists.get("field"))
                    .and_then(serde_json::Value::as_str)
                {
                    fields.insert(field.to_owned());
                }
                if let Some(multi_match) = object.get("multi_match")
                    && let Some(paths) = multi_match
                        .get("fields")
                        .and_then(serde_json::Value::as_array)
                {
                    fields.extend(
                        paths
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .map(strip_boost),
                    );
                }
                for value in object.values() {
                    collect_query_field_paths(value, fields);
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    collect_query_field_paths(value, fields);
                }
            }
            _ => {}
        }
    }

    fn strip_boost(field: &str) -> String {
        field
            .split_once('^')
            .map_or_else(|| field.to_owned(), |(field, _)| field.to_owned())
    }

    #[test]
    fn should_preserve_absent_and_configured_empty_availability_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let absent_document = SearchFilterDocument::try_from(&projection(
            ProductListingSearch::new(Language::En, Currency::Eur),
        ))?;
        assert_eq!(
            None,
            SearchFilterView::try_from(absent_document.clone())?
                .search
                .availability_query
        );
        let absent = serde_json::to_value(absent_document)?;
        assert_eq!(
            Some(&serde_json::Value::Null),
            absent.pointer("/search/availability")
        );

        let mut configured_empty = ProductListingSearch::new(Language::En, Currency::Eur);
        configured_empty.availability_query = Some(ListingAvailabilityQuery {
            any_of: Default::default(),
            orderability: Default::default(),
            include_unspecified: false,
        });
        let document = SearchFilterDocument::try_from(&projection(configured_empty))?;
        let value = serde_json::to_value(&document)?;
        assert_eq!(
            Some(&serde_json::json!({
                "availability": [],
                "orderability": [],
                "includeUnspecified": false
            })),
            value.pointer("/search/availability")
        );
        assert_eq!(
            Some(ListingAvailabilityQuery {
                any_of: Default::default(),
                orderability: Default::default(),
                include_unspecified: false,
            }),
            SearchFilterView::try_from(document)?
                .search
                .availability_query
        );
        Ok(())
    }
}

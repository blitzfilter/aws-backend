use application::error::{BoxError, box_error};
use auction_core::{AuctionId, AuctionTime, AuctionTimeZone};
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
    product_listing_auction::{AuctionMembership, CataloguePosition, LotAuctionTiming, LotNumber},
    product_listing_event::{
        ProductListingChanged, ProductListingDiscovered, ProductListingEventPayload,
        ProductListingEventType, ProductListingImageCount, ProductListingLifecycleChange,
        RehydratedProductListingChanged, RehydratedProductListingDiscovered,
    },
    product_listing_price::ProductListingPrice,
    source_listing_id::SourceListingId,
    title::Title,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

pub(crate) const PRODUCT_LISTING_EVENT_SCHEMA_VERSION: i16 = 1;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProductListingEventCodecError {
    #[error("unsupported ProductListing event type: {event_type}")]
    UnsupportedEventType { event_type: String },
    #[error("unsupported ProductListing event schema version {schema_version} for {event_type}")]
    UnsupportedSchemaVersion {
        event_type: String,
        schema_version: i16,
    },
    #[error("incompatible ProductListing event group {event_group} for {event_type}")]
    IncompatibleEventGroup {
        event_type: String,
        event_group: String,
    },
    #[error("ProductListing event payload is malformed")]
    MalformedPayload {
        #[source]
        source: serde_json::Error,
    },
    #[error("ProductListing event payload is missing field {field}")]
    MissingField { field: String },
    #[error("ProductListing event payload has unknown field {field}")]
    UnknownField { field: String },
    #[error("ProductListing event field {field} is invalid")]
    InvalidField {
        field: &'static str,
        #[source]
        source: BoxError,
    },
    #[error("ProductListing event field {field} is not canonical")]
    NonCanonicalField { field: &'static str },
    #[error("ProductListing event value violates its domain contract")]
    InvalidDomainEvent {
        #[source]
        source: product_listing_core::product_listing_event::RehydrateProductListingEventError,
    },
}

pub(crate) fn encode(
    payload: &ProductListingEventPayload,
) -> Result<Value, ProductListingEventCodecError> {
    match payload {
        ProductListingEventPayload::Discovered(discovered) => {
            serde_json::to_value(DiscoveredDto::try_from(discovered)?).map_err(|source| {
                ProductListingEventCodecError::InvalidField {
                    field: "payload",
                    source: box_error(source),
                }
            })
        }
        ProductListingEventPayload::Changed(changed) => {
            let dto = ChangedDto::try_from(changed)?;
            serde_json::to_value(dto).map_err(|source| {
                ProductListingEventCodecError::InvalidField {
                    field: "payload",
                    source: box_error(source),
                }
            })
        }
    }
}

#[cfg(test)]
pub(crate) fn decode(
    event_type: &str,
    schema_version: i16,
    payload: &Value,
) -> Result<ProductListingEventPayload, ProductListingEventCodecError> {
    decode_domain_payload_with_group(event_type, "DOMAIN", schema_version, payload)
}

fn decode_domain_payload_with_group(
    event_type: &str,
    event_group: &str,
    schema_version: i16,
    payload: &Value,
) -> Result<ProductListingEventPayload, ProductListingEventCodecError> {
    let event_kind = parse_event_type(event_type)?;
    if event_group != "DOMAIN" {
        return Err(ProductListingEventCodecError::IncompatibleEventGroup {
            event_type: event_type.to_owned(),
            event_group: event_group.to_owned(),
        });
    }
    if schema_version != PRODUCT_LISTING_EVENT_SCHEMA_VERSION {
        return Err(ProductListingEventCodecError::UnsupportedSchemaVersion {
            event_type: event_type.to_owned(),
            schema_version,
        });
    }

    match event_kind {
        ProductListingEventType::Discovered => {
            validate_discovered_shape(payload)?;
            serde_json::from_value::<DiscoveredDto>(payload.clone())
                .map_err(|source| ProductListingEventCodecError::MalformedPayload { source })
                .and_then(TryInto::try_into)
        }
        ProductListingEventType::Changed => {
            validate_changed_shape(payload)?;
            serde_json::from_value::<ChangedDto>(payload.clone())
                .map_err(|source| ProductListingEventCodecError::MalformedPayload { source })
                .and_then(ChangedDto::try_into_payload)
        }
    }
}

fn parse_event_type(
    event_type: &str,
) -> Result<ProductListingEventType, ProductListingEventCodecError> {
    match event_type {
        "PRODUCT_LISTING_DISCOVERED" => Ok(ProductListingEventType::Discovered),
        "PRODUCT_LISTING_CHANGED" => Ok(ProductListingEventType::Changed),
        _ => Err(ProductListingEventCodecError::UnsupportedEventType {
            event_type: event_type.to_owned(),
        }),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProductListingPersistedEvent {
    Domain(ProductListingEventType, Box<ProductListingEventPayload>),
    Embedded,
    TranslatedTitles,
}

pub(crate) fn decode_persisted(
    event_type: &str,
    event_group: &str,
    schema_version: i16,
    payload: &Value,
) -> Result<ProductListingPersistedEvent, ProductListingEventCodecError> {
    match event_group {
        "DOMAIN" => {
            let event_kind = match event_type {
                "PRODUCT_LISTING_DISCOVERED" => ProductListingEventType::Discovered,
                "PRODUCT_LISTING_CHANGED" => ProductListingEventType::Changed,
                "ENRICHMENT_EMBEDDED" | "ENRICHMENT_TRANSLATED_TITLES" => {
                    return Err(ProductListingEventCodecError::IncompatibleEventGroup {
                        event_type: event_type.to_owned(),
                        event_group: event_group.to_owned(),
                    });
                }
                _ => {
                    return Err(ProductListingEventCodecError::UnsupportedEventType {
                        event_type: event_type.to_owned(),
                    });
                }
            };
            let payload =
                decode_domain_payload_with_group(event_type, event_group, schema_version, payload)?;
            Ok(ProductListingPersistedEvent::Domain(
                event_kind,
                Box::new(payload),
            ))
        }
        "ENRICHMENT" => {
            if schema_version != PRODUCT_LISTING_EVENT_SCHEMA_VERSION {
                return Err(ProductListingEventCodecError::UnsupportedSchemaVersion {
                    event_type: event_type.to_owned(),
                    schema_version,
                });
            }
            match event_type {
                "ENRICHMENT_EMBEDDED" => {
                    decode_embedded_enrichment(payload)?;
                    Ok(ProductListingPersistedEvent::Embedded)
                }
                "ENRICHMENT_TRANSLATED_TITLES" => {
                    decode_translated_enrichment(payload)?;
                    Ok(ProductListingPersistedEvent::TranslatedTitles)
                }
                "PRODUCT_LISTING_DISCOVERED" | "PRODUCT_LISTING_CHANGED" => {
                    Err(ProductListingEventCodecError::IncompatibleEventGroup {
                        event_type: event_type.to_owned(),
                        event_group: event_group.to_owned(),
                    })
                }
                _ => Err(ProductListingEventCodecError::UnsupportedEventType {
                    event_type: event_type.to_owned(),
                }),
            }
        }
        _ => {
            if matches!(
                event_type,
                "PRODUCT_LISTING_DISCOVERED"
                    | "PRODUCT_LISTING_CHANGED"
                    | "ENRICHMENT_EMBEDDED"
                    | "ENRICHMENT_TRANSLATED_TITLES"
            ) {
                Err(ProductListingEventCodecError::IncompatibleEventGroup {
                    event_type: event_type.to_owned(),
                    event_group: event_group.to_owned(),
                })
            } else {
                Err(ProductListingEventCodecError::UnsupportedEventType {
                    event_type: event_type.to_owned(),
                })
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedEnrichmentDto {
    source_event_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TranslatedEnrichmentDto {
    source_event_id: String,
    source_language: String,
    target_languages: Vec<String>,
}

fn decode_embedded_enrichment(payload: &Value) -> Result<(), ProductListingEventCodecError> {
    let value = serde_json::from_value::<EmbeddedEnrichmentDto>(payload.clone())
        .map_err(|source| ProductListingEventCodecError::MalformedPayload { source })?;
    let source_event_id = parse_uuid(&value.source_event_id, "sourceEventId")?;
    EventId::try_from(source_event_id)
        .map_err(|source| invalid_field_source("sourceEventId", source))?;
    Ok(())
}

fn decode_translated_enrichment(payload: &Value) -> Result<(), ProductListingEventCodecError> {
    let value = serde_json::from_value::<TranslatedEnrichmentDto>(payload.clone())
        .map_err(|source| ProductListingEventCodecError::MalformedPayload { source })?;
    let source_event_id = parse_uuid(&value.source_event_id, "sourceEventId")?;
    EventId::try_from(source_event_id)
        .map_err(|source| invalid_field_source("sourceEventId", source))?;
    parse_language(&value.source_language, "sourceLanguage")?;
    if value.target_languages.is_empty() {
        return Err(invalid_field(
            "targetLanguages",
            "target languages must not be empty",
        ));
    }
    for language in value.target_languages {
        parse_language(&language, "targetLanguages")?;
    }
    Ok(())
}

fn parse_uuid(
    value: &str,
    field: &'static str,
) -> Result<uuid::Uuid, ProductListingEventCodecError> {
    let uuid = value
        .parse::<uuid::Uuid>()
        .map_err(|source| invalid_field_source(field, source))?;
    if uuid.to_string() != value {
        return Err(ProductListingEventCodecError::NonCanonicalField { field });
    }
    Ok(uuid)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiscoveredDto {
    listing_source_id: String,
    source_listing_id: String,
    title: Option<LocalizedTextDto>,
    description: Option<LocalizedTextDto>,
    pricing: PricingDto,
    availability: Option<String>,
    url: String,
    image_count: u64,
    auction: Option<AuctionDto>,
}

impl TryFrom<&ProductListingDiscovered> for DiscoveredDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &ProductListingDiscovered) -> Result<Self, Self::Error> {
        Ok(Self {
            listing_source_id: value.listing_source_id().as_uuid().to_string(),
            source_listing_id: value.source_listing_id().as_ref().to_owned(),
            title: value.title().map(LocalizedTextDto::from),
            description: value.description().map(LocalizedTextDto::from),
            pricing: value.pricing().into(),
            availability: value
                .availability()
                .map(|availability| availability.as_str().to_owned()),
            url: value.url().as_str().to_owned(),
            image_count: value.image_count().value(),
            auction: value.auction().map(TryInto::try_into).transpose()?,
        })
    }
}

impl TryFrom<DiscoveredDto> for ProductListingEventPayload {
    type Error = ProductListingEventCodecError;

    fn try_from(value: DiscoveredDto) -> Result<Self, Self::Error> {
        let listing_source_id = parse_listing_source_id(&value.listing_source_id)?;
        let source_listing_id = parse_source_listing_id(&value.source_listing_id)?;
        let title = value.title.map(LocalizedTextDto::into_title).transpose()?;
        let description = value
            .description
            .map(LocalizedTextDto::into_description)
            .transpose()?;
        let pricing = value.pricing.try_into()?;
        let availability = parse_availability(value.availability, "availability")?;
        let url = parse_url(&value.url, "url")?;
        let image_count = ProductListingImageCount::new(value.image_count);
        let auction = value.auction.map(TryInto::try_into).transpose()?;

        ProductListingEventPayload::rehydrate_discovered(RehydratedProductListingDiscovered {
            listing_source_id,
            source_listing_id,
            title,
            description,
            pricing,
            availability,
            url,
            image_count,
            auction,
        })
        .map_err(|source| ProductListingEventCodecError::InvalidDomainEvent { source })
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ChangedDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pricing: Option<PricingChangesDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    availability: Option<ValueChangeDto<Option<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<ValueChangeDto<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    images: Option<ImageCountChangeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auction: Option<ValueChangeDto<Option<AuctionDto>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lifecycle: Option<LifecycleChangeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sale_observation: Option<SaleObservationChangeDto>,
}

impl TryFrom<&ProductListingChanged> for ChangedDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &ProductListingChanged) -> Result<Self, Self::Error> {
        let pricing = PricingChangesDto {
            price: value
                .price()
                .map(product_listing_price_change_dto)
                .transpose()?,
            price_estimate_min: value
                .price_estimate_min()
                .map(price_change_dto)
                .transpose()?,
            price_estimate_max: value
                .price_estimate_max()
                .map(price_change_dto)
                .transpose()?,
        };
        let changed = Self {
            pricing: (!pricing.is_empty()).then_some(pricing),
            availability: value.availability().map(availability_change_dto),
            url: value.url().map(url_change_dto),
            images: value.image_count().map(ImageCountChangeDto::from),
            auction: value.auction().map(auction_change_dto).transpose()?,
            lifecycle: value.lifecycle().map(LifecycleChangeDto::from),
            sale_observation: value
                .sale_observation()
                .map(sale_observation_change_dto)
                .transpose()?,
        };
        if changed.is_empty() {
            return Err(ProductListingEventCodecError::InvalidDomainEvent {
                source: product_listing_core::product_listing_event::RehydrateProductListingEventError::EmptyChanged,
            });
        }
        Ok(changed)
    }
}

impl ChangedDto {
    fn try_into_payload(self) -> Result<ProductListingEventPayload, ProductListingEventCodecError> {
        let pricing = self
            .pricing
            .map(PricingChangesDto::into_changes)
            .transpose()?;
        let availability = self
            .availability
            .map(|change| {
                Ok((
                    parse_availability(change.previous, "availability.previous")?,
                    parse_availability(change.current, "availability.current")?,
                ))
            })
            .transpose()?;
        let url = self
            .url
            .map(|change| {
                Ok((
                    parse_url(&change.previous, "url.previous")?,
                    parse_url(&change.current, "url.current")?,
                ))
            })
            .transpose()?;
        let images = self.images.map(|change| {
            (
                ProductListingImageCount::new(change.previous_count),
                ProductListingImageCount::new(change.current_count),
            )
        });
        let auction = self
            .auction
            .map(|change| {
                Ok((
                    change.previous.map(TryInto::try_into).transpose()?,
                    change.current.map(TryInto::try_into).transpose()?,
                ))
            })
            .transpose()?;
        let lifecycle = self.lifecycle.map(TryInto::try_into).transpose()?;
        let sale_observation = self
            .sale_observation
            .map(SaleObservationChangeDto::into_transition)
            .transpose()?;

        ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
            price: pricing.as_ref().and_then(|value| value.price),
            price_estimate_min: pricing.as_ref().and_then(|value| value.price_estimate_min),
            price_estimate_max: pricing.as_ref().and_then(|value| value.price_estimate_max),
            availability,
            url,
            images,
            auction,
            lifecycle,
            sale_observation,
        })
        .map_err(|source| ProductListingEventCodecError::InvalidDomainEvent { source })
    }

    fn is_empty(&self) -> bool {
        self.pricing.is_none()
            && self.availability.is_none()
            && self.url.is_none()
            && self.images.is_none()
            && self.auction.is_none()
            && self.lifecycle.is_none()
            && self.sale_observation.is_none()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PricingChangesDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price: Option<ValueChangeDto<Option<ProductListingPriceDto>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<ValueChangeDto<Option<PriceDto>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<ValueChangeDto<Option<PriceDto>>>,
}

impl PricingChangesDto {
    fn is_empty(&self) -> bool {
        self.price.is_none()
            && self.price_estimate_min.is_none()
            && self.price_estimate_max.is_none()
    }

    fn into_changes(self) -> Result<PricingChanges, ProductListingEventCodecError> {
        Ok(PricingChanges {
            price: self
                .price
                .map(|change| parse_product_listing_price_change(change, "pricing.price"))
                .transpose()?,
            price_estimate_min: self
                .price_estimate_min
                .map(|change| parse_price_change(change, "pricing.priceEstimateMin"))
                .transpose()?,
            price_estimate_max: self
                .price_estimate_max
                .map(|change| parse_price_change(change, "pricing.priceEstimateMax"))
                .transpose()?,
        })
    }
}

struct PricingChanges {
    price: Option<(Option<ProductListingPrice>, Option<ProductListingPrice>)>,
    price_estimate_min: Option<(Option<Price>, Option<Price>)>,
    price_estimate_max: Option<(Option<Price>, Option<Price>)>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ValueChangeDto<T> {
    previous: T,
    current: T,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImageCountChangeDto {
    previous_count: u64,
    current_count: u64,
}

impl From<&product_listing_core::product_listing_event::ProductListingImagesChanged>
    for ImageCountChangeDto
{
    fn from(
        value: &product_listing_core::product_listing_event::ProductListingImagesChanged,
    ) -> Self {
        Self {
            previous_count: value.previous_count().value(),
            current_count: value.current_count().value(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleChangeDto {
    transition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_availability: Option<Option<String>>,
}

impl From<&ProductListingLifecycleChange> for LifecycleChangeDto {
    fn from(value: &ProductListingLifecycleChange) -> Self {
        match value {
            ProductListingLifecycleChange::Withdrawn {
                previous_availability,
            } => Self {
                transition: "WITHDRAWN".to_owned(),
                previous_availability: Some(
                    previous_availability.map(|value| value.as_str().to_owned()),
                ),
            },
            ProductListingLifecycleChange::Restored => Self {
                transition: "RESTORED".to_owned(),
                previous_availability: None,
            },
        }
    }
}

impl TryFrom<LifecycleChangeDto> for ProductListingLifecycleChange {
    type Error = ProductListingEventCodecError;

    fn try_from(value: LifecycleChangeDto) -> Result<Self, Self::Error> {
        match value.transition.as_str() {
            "WITHDRAWN" => Ok(Self::Withdrawn {
                previous_availability: parse_availability(
                    value.previous_availability.flatten(),
                    "lifecycle.previousAvailability",
                )?,
            }),
            "RESTORED" => Ok(Self::Restored),
            _ => Err(invalid_field(
                "lifecycle.transition",
                "unknown lifecycle transition",
            )),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaleObservationChangeDto {
    transition: String,
    observation: SaleObservationDto,
}

fn sale_observation_change_dto(
    value: &product_listing_core::product_listing_event::ValueChange<
        Option<ListingSaleObservation>,
    >,
) -> Result<SaleObservationChangeDto, ProductListingEventCodecError> {
    match (value.previous(), value.current()) {
        (None, Some(observation)) => Ok(SaleObservationChangeDto {
            transition: "OBSERVED".to_owned(),
            observation: (*observation).try_into()?,
        }),
        (Some(observation), None) => Ok(SaleObservationChangeDto {
            transition: "RETRACTED".to_owned(),
            observation: (*observation).try_into()?,
        }),
        _ => Err(ProductListingEventCodecError::InvalidDomainEvent {
            source: product_listing_core::product_listing_event::RehydrateProductListingEventError::SaleObservationCorrectionUnsupported,
        }),
    }
}

impl SaleObservationChangeDto {
    fn into_transition(
        self,
    ) -> Result<
        (
            Option<ListingSaleObservation>,
            Option<ListingSaleObservation>,
        ),
        ProductListingEventCodecError,
    > {
        let observation = self.observation.try_into()?;
        match self.transition.as_str() {
            "OBSERVED" => Ok((None, Some(observation))),
            "RETRACTED" => Ok((Some(observation), None)),
            _ => Err(invalid_field(
                "saleObservation.transition",
                "unknown sale observation transition",
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalizedTextDto {
    language: String,
    text: String,
}

impl From<&Localized<Language, Title>> for LocalizedTextDto {
    fn from(value: &Localized<Language, Title>) -> Self {
        Self {
            language: value.localization.as_str().to_owned(),
            text: value.payload.as_ref().to_owned(),
        }
    }
}

impl From<&Localized<Language, Description>> for LocalizedTextDto {
    fn from(value: &Localized<Language, Description>) -> Self {
        Self {
            language: value.localization.as_str().to_owned(),
            text: value.payload.as_ref().to_owned(),
        }
    }
}

impl LocalizedTextDto {
    fn into_title(self) -> Result<Localized<Language, Title>, ProductListingEventCodecError> {
        let language = parse_language(&self.language, "title.language")?;
        let title = Title::from(self.text.clone());
        if title.as_ref() != self.text {
            return Err(ProductListingEventCodecError::NonCanonicalField {
                field: "title.text",
            });
        }
        Ok(Localized::new(language, title))
    }

    fn into_description(
        self,
    ) -> Result<Localized<Language, Description>, ProductListingEventCodecError> {
        let language = parse_language(&self.language, "description.language")?;
        let description = Description::from(self.text.clone());
        if description.as_ref() != self.text {
            return Err(ProductListingEventCodecError::NonCanonicalField {
                field: "description.text",
            });
        }
        Ok(Localized::new(language, description))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PricingDto {
    price: Option<ProductListingPriceDto>,
    price_estimate_min: Option<PriceDto>,
    price_estimate_max: Option<PriceDto>,
}

impl From<ProductListingPricing> for PricingDto {
    fn from(value: ProductListingPricing) -> Self {
        Self {
            price: value.price.map(Into::into),
            price_estimate_min: value.price_estimate_min.map(Into::into),
            price_estimate_max: value.price_estimate_max.map(Into::into),
        }
    }
}

impl TryFrom<PricingDto> for ProductListingPricing {
    type Error = ProductListingEventCodecError;

    fn try_from(value: PricingDto) -> Result<Self, Self::Error> {
        Ok(Self {
            price: parse_product_listing_price(value.price, "pricing.price")?,
            price_estimate_min: parse_price(value.price_estimate_min, "pricing.priceEstimateMin")?,
            price_estimate_max: parse_price(value.price_estimate_max, "pricing.priceEstimateMax")?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PriceDto {
    amount: u64,
    currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum ProductListingPriceDto {
    Monetary { amount: u64, currency: String },
    OnRequest,
}

impl From<ProductListingPrice> for ProductListingPriceDto {
    fn from(value: ProductListingPrice) -> Self {
        match value {
            ProductListingPrice::Monetary(price) => Self::Monetary {
                amount: u64::from(price.monetary_amount),
                currency: price.currency.as_str().to_owned(),
            },
            ProductListingPrice::OnRequest => Self::OnRequest,
        }
    }
}

impl From<Price> for PriceDto {
    fn from(value: Price) -> Self {
        Self {
            amount: u64::from(value.monetary_amount),
            currency: value.currency.as_str().to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuctionDto {
    membership: Option<String>,
    lot_number: Option<String>,
    catalogue_position: Option<u32>,
    timing: Option<LotAuctionTimingDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LotAuctionTimingDto {
    bidding_opens: Option<AuctionTimeDto>,
    scheduled_closes: Option<AuctionTimeDto>,
    reported_closed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "precision",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum AuctionTimeDto {
    Instant {
        instant_at: String,
        source_timezone: Option<String>,
    },
    Date {
        date_on: String,
        source_timezone: Option<String>,
    },
}

impl TryFrom<&ProductListingAuction> for AuctionDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &ProductListingAuction) -> Result<Self, Self::Error> {
        Ok(Self {
            membership: value
                .membership()
                .map(|value| value.auction_id().as_uuid().to_string()),
            lot_number: value.lot_number().map(|value| value.as_str().to_owned()),
            catalogue_position: value.catalogue_position().map(|value| value.value()),
            timing: value.timing().map(TryInto::try_into).transpose()?,
        })
    }
}

impl TryFrom<AuctionDto> for ProductListingAuction {
    type Error = ProductListingEventCodecError;

    fn try_from(value: AuctionDto) -> Result<Self, Self::Error> {
        let membership = value
            .membership
            .map(|value| {
                let uuid = parse_uuid(&value, "auction.membership")?;
                AuctionId::try_from(uuid)
                    .map(AuctionMembership::new)
                    .map_err(|source| invalid_field_source("auction.membership", source))
            })
            .transpose()?;
        let lot_number = value
            .lot_number
            .map(|value| {
                let parsed = LotNumber::try_from(value.as_str())
                    .map_err(|source| invalid_field_source("auction.lotNumber", source))?;
                if parsed.as_str() != value {
                    return Err(ProductListingEventCodecError::NonCanonicalField {
                        field: "auction.lotNumber",
                    });
                }
                Ok(parsed)
            })
            .transpose()?;
        let catalogue_position = value
            .catalogue_position
            .map(|value| {
                CataloguePosition::new(value)
                    .map_err(|source| invalid_field_source("auction.cataloguePosition", source))
            })
            .transpose()?;
        let timing = value.timing.map(TryInto::try_into).transpose()?;
        Ok(ProductListingAuction::new(
            membership,
            lot_number,
            catalogue_position,
            timing,
        ))
    }
}

impl TryFrom<&LotAuctionTiming> for LotAuctionTimingDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &LotAuctionTiming) -> Result<Self, Self::Error> {
        Ok(Self {
            bidding_opens: value.bidding_opens().map(TryInto::try_into).transpose()?,
            scheduled_closes: value
                .scheduled_closes()
                .map(TryInto::try_into)
                .transpose()?,
            reported_closed_at: value
                .reported_closed_at()
                .map(|value| format_timestamp(value, "auction.timing.reportedClosedAt"))
                .transpose()?,
        })
    }
}

impl TryFrom<LotAuctionTimingDto> for LotAuctionTiming {
    type Error = ProductListingEventCodecError;

    fn try_from(value: LotAuctionTimingDto) -> Result<Self, Self::Error> {
        LotAuctionTiming::new(
            value.bidding_opens.map(TryInto::try_into).transpose()?,
            value.scheduled_closes.map(TryInto::try_into).transpose()?,
            value
                .reported_closed_at
                .as_deref()
                .map(|value| parse_timestamp(value, "auction.timing.reportedClosedAt"))
                .transpose()?,
        )
        .map_err(|source| invalid_field_source("auction.timing", source))
    }
}

impl TryFrom<&AuctionTime> for AuctionTimeDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: &AuctionTime) -> Result<Self, Self::Error> {
        match value {
            AuctionTime::Instant {
                at,
                source_timezone,
            } => Ok(Self::Instant {
                instant_at: format_timestamp(*at, "auction.timing.instantAt")?,
                source_timezone: source_timezone
                    .as_ref()
                    .map(|value| value.as_str().to_owned()),
            }),
            AuctionTime::Date {
                on,
                source_timezone,
            } => Ok(Self::Date {
                date_on: on.to_string(),
                source_timezone: source_timezone
                    .as_ref()
                    .map(|value| value.as_str().to_owned()),
            }),
        }
    }
}

impl TryFrom<AuctionTimeDto> for AuctionTime {
    type Error = ProductListingEventCodecError;

    fn try_from(value: AuctionTimeDto) -> Result<Self, Self::Error> {
        match value {
            AuctionTimeDto::Instant {
                instant_at,
                source_timezone,
            } => Ok(AuctionTime::instant(
                parse_timestamp(&instant_at, "auction.timing.instantAt")?,
                parse_auction_timezone(source_timezone, "auction.timing.sourceTimezone")?,
            )),
            AuctionTimeDto::Date {
                date_on,
                source_timezone,
            } => Ok(AuctionTime::date(
                parse_date(&date_on, "auction.timing.dateOn")?,
                parse_auction_timezone(source_timezone, "auction.timing.sourceTimezone")?,
            )),
        }
    }
}

fn parse_auction_timezone(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<AuctionTimeZone>, ProductListingEventCodecError> {
    value
        .map(|value| {
            AuctionTimeZone::try_from(value).map_err(|source| invalid_field_source(field, source))
        })
        .transpose()
}

#[allow(deprecated)]
fn parse_date(
    value: &str,
    field: &'static str,
) -> Result<time::Date, ProductListingEventCodecError> {
    let format = time::format_description::parse("[year]-[month]-[day]")
        .map_err(|source| invalid_field_source(field, source))?;
    let date =
        time::Date::parse(value, &format).map_err(|source| invalid_field_source(field, source))?;
    if date.to_string() != value {
        return Err(ProductListingEventCodecError::NonCanonicalField { field });
    }
    Ok(date)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaleObservationDto {
    observed_at: String,
    fx_rate_id: String,
}

impl TryFrom<ListingSaleObservation> for SaleObservationDto {
    type Error = ProductListingEventCodecError;

    fn try_from(value: ListingSaleObservation) -> Result<Self, Self::Error> {
        Ok(Self {
            observed_at: format_timestamp(value.observed_at(), "saleObservation.observedAt")?,
            fx_rate_id: value.fx_rate_id().as_uuid().to_string(),
        })
    }
}

impl TryFrom<SaleObservationDto> for ListingSaleObservation {
    type Error = ProductListingEventCodecError;

    fn try_from(value: SaleObservationDto) -> Result<Self, Self::Error> {
        let fx_rate_uuid = parse_uuid(&value.fx_rate_id, "saleObservation.fxRateId")?;
        let fx_rate_id = fxrate_core::FxRateId::try_from(fx_rate_uuid)
            .map_err(|source| invalid_field_source("saleObservation.fxRateId", source))?;
        Ok(Self::new(
            parse_timestamp(&value.observed_at, "saleObservation.observedAt")?,
            fx_rate_id,
        ))
    }
}

fn product_listing_price_change_dto(
    value: &product_listing_core::product_listing_event::ValueChange<Option<ProductListingPrice>>,
) -> Result<ValueChangeDto<Option<ProductListingPriceDto>>, ProductListingEventCodecError> {
    Ok(ValueChangeDto {
        previous: value.previous().map(ProductListingPriceDto::from),
        current: value.current().map(ProductListingPriceDto::from),
    })
}

fn price_change_dto(
    value: &product_listing_core::product_listing_event::ValueChange<Option<Price>>,
) -> Result<ValueChangeDto<Option<PriceDto>>, ProductListingEventCodecError> {
    Ok(ValueChangeDto {
        previous: value.previous().map(PriceDto::from),
        current: value.current().map(PriceDto::from),
    })
}

fn availability_change_dto(
    value: &product_listing_core::product_listing_event::ValueChange<Option<ListingAvailability>>,
) -> ValueChangeDto<Option<String>> {
    ValueChangeDto {
        previous: value
            .previous()
            .map(|availability| availability.as_str().to_owned()),
        current: value
            .current()
            .map(|availability| availability.as_str().to_owned()),
    }
}

fn url_change_dto(
    value: &product_listing_core::product_listing_event::ValueChange<Url>,
) -> ValueChangeDto<String> {
    ValueChangeDto {
        previous: value.previous().as_str().to_owned(),
        current: value.current().as_str().to_owned(),
    }
}

fn auction_change_dto(
    value: &product_listing_core::product_listing_event::ValueChange<Option<ProductListingAuction>>,
) -> Result<ValueChangeDto<Option<AuctionDto>>, ProductListingEventCodecError> {
    Ok(ValueChangeDto {
        previous: value
            .previous()
            .as_ref()
            .map(TryInto::try_into)
            .transpose()?,
        current: value
            .current()
            .as_ref()
            .map(TryInto::try_into)
            .transpose()?,
    })
}

fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ProductListingEventCodecError> {
    let uuid = parse_uuid(value, "listingSourceId")?;
    ListingSourceId::try_from(uuid)
        .map_err(|source| invalid_field_source("listingSourceId", source))
}

fn parse_source_listing_id(value: &str) -> Result<SourceListingId, ProductListingEventCodecError> {
    let id = SourceListingId::try_from(value.to_owned())
        .map_err(|source| invalid_field_source("sourceListingId", source))?;
    if id.as_ref() != value {
        return Err(ProductListingEventCodecError::NonCanonicalField {
            field: "sourceListingId",
        });
    }
    Ok(id)
}

fn parse_language(
    value: &str,
    field: &'static str,
) -> Result<Language, ProductListingEventCodecError> {
    Language::from_code(value).ok_or_else(|| invalid_field(field, "unknown language code"))
}

fn parse_availability(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<ListingAvailability>, ProductListingEventCodecError> {
    value
        .map(|value| {
            ListingAvailability::from_code(&value)
                .ok_or_else(|| invalid_field(field, "unknown availability code"))
        })
        .transpose()
}

fn parse_price(
    value: Option<PriceDto>,
    field: &'static str,
) -> Result<Option<Price>, ProductListingEventCodecError> {
    value
        .map(|value| {
            let currency = Currency::from_code(&value.currency)
                .ok_or_else(|| invalid_field(field, "unknown currency code"))?;
            if currency.as_str() != value.currency {
                return Err(ProductListingEventCodecError::NonCanonicalField { field });
            }
            Ok(Price::new(MonetaryAmount::from(value.amount), currency))
        })
        .transpose()
}

fn parse_product_listing_price(
    value: Option<ProductListingPriceDto>,
    field: &'static str,
) -> Result<Option<ProductListingPrice>, ProductListingEventCodecError> {
    value
        .map(|value| match value {
            ProductListingPriceDto::Monetary { amount, currency } => {
                let parsed_currency = Currency::from_code(&currency)
                    .ok_or_else(|| invalid_field(field, "unknown currency code"))?;
                if parsed_currency.as_str() != currency {
                    return Err(ProductListingEventCodecError::NonCanonicalField { field });
                }
                Ok(ProductListingPrice::Monetary(Price::new(
                    MonetaryAmount::from(amount),
                    parsed_currency,
                )))
            }
            ProductListingPriceDto::OnRequest => Ok(ProductListingPrice::OnRequest),
        })
        .transpose()
}

fn parse_product_listing_price_change(
    value: ValueChangeDto<Option<ProductListingPriceDto>>,
    field: &'static str,
) -> Result<(Option<ProductListingPrice>, Option<ProductListingPrice>), ProductListingEventCodecError>
{
    Ok((
        parse_product_listing_price(value.previous, field)?,
        parse_product_listing_price(value.current, field)?,
    ))
}

fn parse_price_change(
    value: ValueChangeDto<Option<PriceDto>>,
    field: &'static str,
) -> Result<(Option<Price>, Option<Price>), ProductListingEventCodecError> {
    Ok((
        parse_price(value.previous, field)?,
        parse_price(value.current, field)?,
    ))
}

fn parse_url(value: &str, field: &'static str) -> Result<Url, ProductListingEventCodecError> {
    let url = Url::parse(value).map_err(|source| invalid_field_source(field, source))?;
    if url.as_str() != value {
        return Err(ProductListingEventCodecError::NonCanonicalField { field });
    }
    Ok(url)
}

fn format_timestamp(
    value: OffsetDateTime,
    field: &'static str,
) -> Result<String, ProductListingEventCodecError> {
    value
        .format(&Rfc3339)
        .map_err(|source| invalid_field_source(field, source))
}

fn parse_timestamp(
    value: &str,
    field: &'static str,
) -> Result<OffsetDateTime, ProductListingEventCodecError> {
    let timestamp = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|source| invalid_field_source(field, source))?;
    let canonical = format_timestamp(timestamp, field)?;
    if canonical != value {
        return Err(ProductListingEventCodecError::NonCanonicalField { field });
    }
    Ok(timestamp)
}

fn validate_discovered_shape(value: &Value) -> Result<(), ProductListingEventCodecError> {
    let object = required_object(value, "payload")?;
    require_exact_keys(
        object,
        &[
            "listingSourceId",
            "sourceListingId",
            "title",
            "description",
            "pricing",
            "availability",
            "url",
            "imageCount",
            "auction",
        ],
        "payload",
    )?;
    validate_localized_shape(
        object
            .get("title")
            .ok_or_else(|| missing_field("payload.title"))?,
        "title",
    )?;
    validate_localized_shape(
        object
            .get("description")
            .ok_or_else(|| missing_field("payload.description"))?,
        "description",
    )?;

    let pricing = required_object_member(object, "pricing", "payload")?;
    require_exact_keys(
        pricing,
        &["price", "priceEstimateMin", "priceEstimateMax"],
        "pricing",
    )?;
    validate_product_listing_price_shape(
        pricing
            .get("price")
            .ok_or_else(|| missing_field("pricing.price"))?,
        "pricing.price",
    )?;
    for field in ["priceEstimateMin", "priceEstimateMax"] {
        validate_price_shape(
            pricing
                .get(field)
                .ok_or_else(|| missing_field(format!("pricing.{field}")))?,
            "pricing",
        )?;
    }

    validate_auction_shape(
        object
            .get("auction")
            .ok_or_else(|| missing_field("payload.auction"))?,
        "auction",
    )?;
    Ok(())
}

fn validate_changed_shape(value: &Value) -> Result<(), ProductListingEventCodecError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_field("payload", "changed payload must be an object"))?;
    if object.is_empty() {
        return Err(invalid_field(
            "payload",
            "changed payload must not be empty",
        ));
    }

    for (field, raw) in object {
        match field.as_str() {
            "pricing" => {
                let pricing = required_object(raw, "pricing")?;
                if pricing.is_empty() {
                    return Err(invalid_field(
                        "pricing",
                        "pricing changes must not be empty",
                    ));
                }
                for (pricing_field, pricing_change) in pricing {
                    match pricing_field.as_str() {
                        "price" => {
                            validate_product_listing_price_change_shape(
                                pricing_change,
                                "pricing.price",
                            )?;
                        }
                        "priceEstimateMin" | "priceEstimateMax" => {
                            validate_price_change_shape(pricing_change, "pricing change")?;
                        }
                        _ => {
                            return Err(invalid_field(
                                "pricing",
                                "unknown pricing change dimension",
                            ));
                        }
                    }
                }
            }
            "availability" | "url" => {
                validate_value_change_shape(raw, "changed dimension")?;
            }
            "images" => {
                let images = required_object(raw, "images")?;
                require_exact_keys(images, &["previousCount", "currentCount"], "images")?;
            }
            "auction" => {
                let change = required_object(raw, "auction")?;
                require_exact_keys(change, &["previous", "current"], "auction change")?;
                validate_auction_shape(
                    change
                        .get("previous")
                        .ok_or_else(|| missing_field("auction.previous"))?,
                    "auction.previous",
                )?;
                validate_auction_shape(
                    change
                        .get("current")
                        .ok_or_else(|| missing_field("auction.current"))?,
                    "auction.current",
                )?;
            }
            "saleObservation" => validate_sale_observation_shape(raw)?,
            "lifecycle" => validate_lifecycle_shape(raw)?,
            _ => {
                return Err(invalid_field("payload", "unknown changed event dimension"));
            }
        }
    }
    Ok(())
}

fn validate_auction_shape(
    value: &Value,
    context: &'static str,
) -> Result<(), ProductListingEventCodecError> {
    if value.is_null() {
        return Ok(());
    }
    let auction = required_object(value, context)?;
    require_exact_keys(
        auction,
        &["membership", "lotNumber", "cataloguePosition", "timing"],
        context,
    )?;
    let timing = auction
        .get("timing")
        .ok_or_else(|| missing_field(format!("{context}.timing")))?;
    if timing.is_null() {
        return Ok(());
    }
    let timing = required_object(timing, "auction timing")?;
    require_exact_keys(
        timing,
        &["biddingOpens", "scheduledCloses", "reportedClosedAt"],
        "auction timing",
    )?;
    for field in ["biddingOpens", "scheduledCloses"] {
        validate_auction_time_shape(
            timing
                .get(field)
                .ok_or_else(|| missing_field(format!("auction timing.{field}")))?,
        )?;
    }
    Ok(())
}

fn validate_auction_time_shape(value: &Value) -> Result<(), ProductListingEventCodecError> {
    if value.is_null() {
        return Ok(());
    }
    let time = required_object(value, "auction time")?;
    let precision = time
        .get("precision")
        .and_then(Value::as_str)
        .ok_or_else(|| missing_field("auction time.precision"))?;
    match precision {
        "INSTANT" => require_exact_keys(
            time,
            &["precision", "instantAt", "sourceTimezone"],
            "auction time",
        ),
        "DATE" => require_exact_keys(
            time,
            &["precision", "dateOn", "sourceTimezone"],
            "auction time",
        ),
        _ => Err(invalid_field(
            "auction time.precision",
            "unknown auction time precision",
        )),
    }
}

fn validate_lifecycle_shape(value: &Value) -> Result<(), ProductListingEventCodecError> {
    let object = required_object(value, "lifecycle")?;
    let transition = object
        .get("transition")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_field("lifecycle.transition", "transition must be a string"))?;
    let expected = match transition {
        "WITHDRAWN" => &["transition", "previousAvailability"][..],
        "RESTORED" => &["transition"][..],
        _ => {
            return Err(invalid_field(
                "lifecycle.transition",
                "unknown lifecycle transition",
            ));
        }
    };
    require_exact_keys(object, expected, "lifecycle")?;
    if transition == "WITHDRAWN" {
        let previous = object
            .get("previousAvailability")
            .ok_or_else(|| missing_field("lifecycle.previousAvailability"))?;
        if !previous.is_null() && !previous.is_string() {
            return Err(invalid_field(
                "lifecycle.previousAvailability",
                "previous availability must be nullable text",
            ));
        }
    }
    Ok(())
}

fn validate_value_change_shape(
    value: &Value,
    field: &'static str,
) -> Result<(), ProductListingEventCodecError> {
    let object = required_object(value, field)?;
    require_exact_keys(object, &["previous", "current"], field)
}

fn validate_price_change_shape(
    value: &Value,
    field: &'static str,
) -> Result<(), ProductListingEventCodecError> {
    let object = required_object(value, field)?;
    require_exact_keys(object, &["previous", "current"], field)?;
    for endpoint in ["previous", "current"] {
        validate_price_shape(
            object
                .get(endpoint)
                .ok_or_else(|| missing_field(format!("{field}.{endpoint}")))?,
            field,
        )?;
    }
    Ok(())
}

fn validate_product_listing_price_change_shape(
    value: &Value,
    field: &'static str,
) -> Result<(), ProductListingEventCodecError> {
    let object = required_object(value, field)?;
    require_exact_keys(object, &["previous", "current"], field)?;
    for endpoint in ["previous", "current"] {
        validate_product_listing_price_shape(
            object
                .get(endpoint)
                .ok_or_else(|| missing_field(format!("{field}.{endpoint}")))?,
            field,
        )?;
    }
    Ok(())
}

fn validate_product_listing_price_shape(
    value: &Value,
    field: &'static str,
) -> Result<(), ProductListingEventCodecError> {
    if value.is_null() {
        return Ok(());
    }
    let object = required_object(value, field)?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| missing_field(format!("{field}.type")))?;
    match kind {
        "MONETARY" => require_exact_keys(object, &["type", "amount", "currency"], field),
        "ON_REQUEST" => require_exact_keys(object, &["type"], field),
        _ => Err(invalid_field(field, "unknown ProductListing price type")),
    }
}

fn validate_price_shape(
    value: &Value,
    field: &'static str,
) -> Result<(), ProductListingEventCodecError> {
    if value.is_null() {
        return Ok(());
    }
    let object = required_object(value, field)?;
    require_exact_keys(object, &["amount", "currency"], field)
}

fn validate_localized_shape(
    value: &Value,
    field: &'static str,
) -> Result<(), ProductListingEventCodecError> {
    if value.is_null() {
        return Ok(());
    }
    let object = required_object(value, field)?;
    require_exact_keys(object, &["language", "text"], field)
}

fn validate_sale_observation_shape(value: &Value) -> Result<(), ProductListingEventCodecError> {
    let object = required_object(value, "saleObservation")?;
    require_exact_keys(object, &["transition", "observation"], "saleObservation")?;
    let observation = required_object_member(object, "observation", "saleObservation")?;
    require_exact_keys(
        observation,
        &["observedAt", "fxRateId"],
        "saleObservation.observation",
    )
}

fn required_object_member<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    context: &str,
) -> Result<&'a serde_json::Map<String, Value>, ProductListingEventCodecError> {
    let value = object
        .get(field)
        .ok_or_else(|| missing_field(format!("{context}.{field}")))?;
    required_object(value, "nested object")
}

fn require_exact_keys(
    object: &serde_json::Map<String, Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), ProductListingEventCodecError> {
    for field in expected {
        if !object.contains_key(*field) {
            return Err(missing_field(format!("{context}.{field}")));
        }
    }
    for field in object.keys() {
        if !expected.contains(&field.as_str()) {
            return Err(ProductListingEventCodecError::UnknownField {
                field: format!("{context}.{field}"),
            });
        }
    }
    Ok(())
}

fn missing_field(field: impl Into<String>) -> ProductListingEventCodecError {
    ProductListingEventCodecError::MissingField {
        field: field.into(),
    }
}

fn required_object<'a>(
    value: &'a Value,
    field: &'static str,
) -> Result<&'a serde_json::Map<String, Value>, ProductListingEventCodecError> {
    value
        .as_object()
        .ok_or_else(|| invalid_field(field, "field must be an object"))
}

fn invalid_field(field: &'static str, message: &'static str) -> ProductListingEventCodecError {
    invalid_field_source(field, std::io::Error::other(message))
}

fn invalid_field_source<E>(field: &'static str, source: E) -> ProductListingEventCodecError
where
    E: std::error::Error + Send + Sync + 'static,
{
    ProductListingEventCodecError::InvalidField {
        field,
        source: box_error(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auction_core::AuctionTime;
    use listing_source_core::ListingSourceId;
    use localization::{Language, Localized};
    use money::{Currency, MonetaryAmount};
    use product_listing_core::{
        description::Description,
        listing_availability::ListingAvailability,
        product_listing::{ListingSaleObservation, ProductListingAuction, ProductListingPricing},
        product_listing_auction::{CataloguePosition, LotAuctionTiming, LotNumber},
        product_listing_event::{
            RehydratedProductListingChanged, RehydratedProductListingDiscovered,
        },
        source_listing_id::SourceListingId,
        title::Title,
    };
    use serde_json::json;
    use time::Duration;

    #[test]
    fn should_round_trip_discovered_payload_without_kind_or_image_urls() {
        let payload = discovered_payload();
        let encoded = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));

        assert!(encoded.get("kind").is_none());
        assert_eq!(Some(1), encoded.get("imageCount").and_then(Value::as_u64));
        assert!(!encoded.to_string().contains("image-a.jpg"));
        assert_eq!(
            payload,
            decode(
                ProductListingEventType::Discovered.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                &encoded,
            )
            .unwrap_or_else(|error| panic!("decode: {error}")),
        );
    }

    #[test]
    fn should_round_trip_changed_payload_with_all_dimensions() {
        let payload = changed_payload_with_all_dimensions();
        let encoded = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));

        assert_eq!(
            payload,
            decode(
                ProductListingEventType::Changed.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                &encoded,
            )
            .unwrap_or_else(|error| panic!("decode: {error}")),
        );
    }

    #[test]
    fn should_encode_storage_object_ids_as_exact_raw_uuids() {
        let discovered = encode(&discovered_payload())
            .unwrap_or_else(|error| panic!("encode discovery: {error}"));
        let changed = encode(&changed_payload_with_all_dimensions())
            .unwrap_or_else(|error| panic!("encode change: {error}"));

        assert_eq!(
            Some("01900000-0000-7000-8000-000000000001"),
            discovered
                .pointer("/listingSourceId")
                .and_then(Value::as_str)
        );
        assert_eq!(
            Some("01900000-0000-7000-8000-000000000002"),
            changed
                .pointer("/saleObservation/observation/fxRateId")
                .and_then(Value::as_str)
        );
        assert!(!discovered.to_string().contains("ls_"));
        assert!(!changed.to_string().contains("fx_"));
    }

    #[test]
    fn should_reject_uuid_v4_object_ids_in_storage_json() {
        let invalid_v4 = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        let mut discovered = canonical_discovery_value();
        discovered["listingSourceId"] = json!(invalid_v4);
        let mut changed = encode(&changed_payload_with_all_dimensions())
            .unwrap_or_else(|error| panic!("encode change: {error}"));
        changed["saleObservation"]["observation"]["fxRateId"] = json!(invalid_v4);

        assert!(decode("PRODUCT_LISTING_DISCOVERED", 1, &discovered).is_err());
        assert!(decode("PRODUCT_LISTING_CHANGED", 1, &changed).is_err());
        assert!(
            decode_persisted(
                "ENRICHMENT_EMBEDDED",
                "ENRICHMENT",
                1,
                &json!({"sourceEventId": invalid_v4}),
            )
            .is_err()
        );
    }

    #[test]
    fn should_reject_noncanonical_fx_rate_ids_in_storage_json() {
        for fx_rate_id in [
            "01900000-0000-7000-8000-00000000ABCD",
            "0190000000007000800000000000abcd",
        ] {
            let mut changed = encode(&changed_payload_with_all_dimensions())
                .unwrap_or_else(|error| panic!("encode change: {error}"));
            changed["saleObservation"]["observation"]["fxRateId"] = json!(fx_rate_id);

            assert!(matches!(
                decode("PRODUCT_LISTING_CHANGED", 1, &changed),
                Err(ProductListingEventCodecError::NonCanonicalField {
                    field: "saleObservation.fxRateId"
                })
            ));
        }
    }

    #[test]
    fn should_round_trip_withdrawal_with_previous_availability() {
        let payload = withdrawal_payload();
        let encoded = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));

        assert_eq!(
            payload,
            decode(
                ProductListingEventType::Changed.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                &encoded,
            )
            .unwrap_or_else(|error| panic!("decode: {error}")),
        );
    }

    #[test]
    fn should_round_trip_retracted_sale_observation_change() {
        let observation =
            ListingSaleObservation::new(OffsetDateTime::UNIX_EPOCH, fxrate_core::FxRateId::new());
        let payload = changed_payload(RehydratedProductListingChanged {
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            availability: None,
            url: None,
            images: None,
            auction: None,
            lifecycle: None,
            sale_observation: Some((Some(observation), None)),
        });
        let encoded = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));

        assert_eq!(
            payload,
            decode(
                ProductListingEventType::Changed.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                &encoded,
            )
            .unwrap_or_else(|error| panic!("decode: {error}")),
        );
    }

    #[test]
    fn should_reject_unknown_fields_versions_types_and_empty_changes() {
        let payload = discovered_payload();
        let mut unknown = encode(&payload).unwrap_or_else(|error| panic!("encode: {error}"));
        unknown["kind"] = json!("discovered");

        for (event_type, version, payload) in [
            (
                ProductListingEventType::Discovered.as_str(),
                PRODUCT_LISTING_EVENT_SCHEMA_VERSION,
                unknown,
            ),
            (ProductListingEventType::Discovered.as_str(), 2, json!({})),
            ("PRODUCT_LISTING_UNKNOWN", 1, json!({})),
            (ProductListingEventType::Changed.as_str(), 1, json!({})),
        ] {
            assert!(decode(event_type, version, &payload).is_err());
        }
    }

    #[test]
    fn should_reject_malformed_nested_changes_without_aggregate_reconstruction() {
        for payload in [
            json!({"pricing": {"price": {}}}),
            json!({"availability": {}}),
            json!({"images": {}}),
            json!({"unknown": {"previous": null, "current": null}}),
        ] {
            assert!(decode("PRODUCT_LISTING_CHANGED", 1, &payload).is_err());
        }
    }

    #[test]
    fn should_accept_same_count_image_replacement() {
        let payload = json!({"images": {"previousCount": 2, "currentCount": 2}});
        let decoded = decode("PRODUCT_LISTING_CHANGED", 1, &payload)
            .unwrap_or_else(|error| panic!("decode: {error}"));
        let ProductListingEventPayload::Changed(changed) = decoded else {
            panic!("expected changed payload");
        };
        assert_eq!(
            Some(2),
            changed
                .image_count()
                .map(|value| value.previous_count().value())
        );
        assert_eq!(
            Some(2),
            changed
                .image_count()
                .map(|value| value.current_count().value())
        );
    }

    #[test]
    fn should_reject_incomplete_lifecycle_change_shape() {
        for payload in [
            json!({"lifecycle": {"transition": "WITHDRAWN"}}),
            json!({"lifecycle": {"transition": "RESTORED", "previousAvailability": null}}),
        ] {
            assert!(decode("PRODUCT_LISTING_CHANGED", 1, &payload).is_err());
        }
    }

    #[test]
    fn should_decode_large_image_counts_without_image_allocation() {
        let payload = json!({
            "listingSourceId": "01900000-0000-7000-8000-000000000001",
            "sourceListingId": "fixture-source-id",
            "title": null,
            "description": null,
            "pricing": {"price": null, "priceEstimateMin": null, "priceEstimateMax": null},
            "availability": null,
            "url": "https://example.test/product",
            "imageCount": u64::MAX,
            "auction": null
        });
        let decoded = decode("PRODUCT_LISTING_DISCOVERED", 1, &payload)
            .unwrap_or_else(|error| panic!("decode: {error}"));
        let ProductListingEventPayload::Discovered(discovered) = decoded else {
            panic!("expected discovered payload");
        };
        assert_eq!(u64::MAX, discovered.image_count().value());
    }

    #[test]
    fn should_reject_noncanonical_discovery_strings() {
        let mut payload =
            encode(&discovered_payload()).unwrap_or_else(|error| panic!("encode: {error}"));
        payload["sourceListingId"] = json!(" source-listing ");
        assert!(matches!(
            decode("PRODUCT_LISTING_DISCOVERED", 1, &payload),
            Err(ProductListingEventCodecError::NonCanonicalField {
                field: "sourceListingId"
            })
        ));

        let mut payload =
            encode(&discovered_payload()).unwrap_or_else(|error| panic!("encode: {error}"));
        payload["title"]["text"] = json!(" codec title. ");
        assert!(matches!(
            decode("PRODUCT_LISTING_DISCOVERED", 1, &payload),
            Err(ProductListingEventCodecError::NonCanonicalField {
                field: "title.text"
            })
        ));
    }

    #[test]
    fn should_reject_product_listing_v1_negative_contract_matrix() {
        let mut unknown_discovery = canonical_discovery_value();
        unknown_discovery["unexpected"] = json!(true);

        let mut omitted_title = canonical_discovery_value();
        if let Some(object) = omitted_title.as_object_mut() {
            object.remove("title");
        }

        let mut omitted_price = canonical_discovery_value();
        if let Some(object) = omitted_price
            .get_mut("pricing")
            .and_then(Value::as_object_mut)
        {
            object.remove("price");
        }

        let mut omitted_lot_bidding_opens = canonical_discovery_value();
        omitted_lot_bidding_opens["auction"] = json!({
            "lotNumber": null,
            "cataloguePosition": null,
            "timing": null
        });
        if let Some(object) = omitted_lot_bidding_opens
            .get_mut("auction")
            .and_then(Value::as_object_mut)
        {
            object.remove("lotNumber");
        }

        let cases = vec![
            (
                "unknown discovery field",
                "PRODUCT_LISTING_DISCOVERED",
                unknown_discovery,
            ),
            (
                "omitted nullable discovery field",
                "PRODUCT_LISTING_DISCOVERED",
                omitted_title,
            ),
            (
                "omitted pricing field",
                "PRODUCT_LISTING_DISCOVERED",
                omitted_price,
            ),
            (
                "omitted auction field",
                "PRODUCT_LISTING_DISCOVERED",
                omitted_lot_bidding_opens,
            ),
            ("unknown localized field", "PRODUCT_LISTING_DISCOVERED", {
                let mut value = canonical_discovery_value();
                value["title"] = json!({
                    "language": "en",
                    "text": "Codec title",
                    "unexpected": true
                });
                value
            }),
            (
                "noncanonical discovery URL",
                "PRODUCT_LISTING_DISCOVERED",
                {
                    let mut value = canonical_discovery_value();
                    value["url"] = json!("https://example.com:443/product");
                    value
                },
            ),
            (
                "unparsable changed URL",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "url": {"previous": "not a URL", "current": "https://example.com/new"}
                }),
            ),
            (
                "noncanonical changed URL",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "url": {
                        "previous": "https://example.com/old",
                        "current": "https://example.com:443/new"
                    }
                }),
            ),
            (
                "unknown image field",
                "PRODUCT_LISTING_CHANGED",
                json!({"images": {"previousCount": 1, "currentCount": 2, "unexpected": true}}),
            ),
            (
                "unknown value change field",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "availability": {"previous": null, "current": "AVAILABLE", "unexpected": true}
                }),
            ),
            (
                "unknown price field",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "pricing": {
                        "price": {
                            "previous": {"amount": 1, "currency": "EUR", "unexpected": true},
                            "current": null
                        }
                    }
                }),
            ),
            (
                "invalid auction timestamp",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "auction": {
                        "previous": {
                            "lotNumber": null,
                            "cataloguePosition": null,
                            "timing": {
                                "biddingOpens": {"precision": "INSTANT", "instantAt": "not a timestamp", "sourceTimezone": null},
                                "scheduledCloses": null,
                                "reportedClosedAt": null
                            }
                        },
                        "current": null
                    }
                }),
            ),
            (
                "auction opens after close",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "auction": {
                        "previous": {
                            "lotNumber": null,
                            "cataloguePosition": null,
                            "timing": {
                                "biddingOpens": {"precision": "INSTANT", "instantAt": "2025-01-02T00:00:00Z", "sourceTimezone": null},
                                "scheduledCloses": {"precision": "INSTANT", "instantAt": "2025-01-01T00:00:00Z", "sourceTimezone": null},
                                "reportedClosedAt": null
                            }
                        },
                        "current": null
                    }
                }),
            ),
            (
                "invalid sale observation timestamp",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "saleObservation": {
                        "transition": "OBSERVED",
                        "observation": {"observedAt": "yesterday", "fxRateId": "01900000-0000-7000-8000-000000000001"}
                    }
                }),
            ),
            (
                "invalid sale observation FX ID",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "saleObservation": {
                        "transition": "OBSERVED",
                        "observation": {"observedAt": "1970-01-01T00:00:00Z", "fxRateId": "not-a-uuid"}
                    }
                }),
            ),
            (
                "withdrawal with current availability",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "availability": {"previous": null, "current": "AVAILABLE"},
                    "lifecycle": {"transition": "WITHDRAWN", "previousAvailability": null}
                }),
            ),
            (
                "restoration with previous availability",
                "PRODUCT_LISTING_CHANGED",
                json!({
                    "availability": {"previous": "AVAILABLE", "current": null},
                    "lifecycle": {"transition": "RESTORED"}
                }),
            ),
        ];

        for (name, event_type, payload) in cases {
            assert!(decode(event_type, 1, &payload).is_err(), "{name}");
        }
    }

    #[test]
    fn should_accept_product_listing_v1_positive_contract_matrix() {
        let valid_changed = json!({
            "pricing": {"price": {"previous": null, "current": {"type": "MONETARY", "amount": 10, "currency": "EUR"}}},
            "availability": {"previous": null, "current": "AVAILABLE"},
            "images": {"previousCount": 1, "currentCount": 1}
        });
        let withdrawal = json!({
            "availability": {"previous": "AVAILABLE", "current": null},
            "lifecycle": {"transition": "WITHDRAWN", "previousAvailability": "AVAILABLE"}
        });
        let restoration = json!({
            "availability": {"previous": null, "current": "AVAILABLE"},
            "lifecycle": {"transition": "RESTORED"}
        });

        for payload in [
            canonical_discovery_value(),
            json!({"availability": {"previous": null, "current": "AVAILABLE"}}),
            json!({"images": {"previousCount": 2, "currentCount": 2}}),
            valid_changed,
            withdrawal,
            restoration,
        ] {
            let event_type = if payload.get("listingSourceId").is_some() {
                "PRODUCT_LISTING_DISCOVERED"
            } else {
                "PRODUCT_LISTING_CHANGED"
            };
            assert!(decode(event_type, 1, &payload).is_ok(), "{payload}");
        }

        assert!(matches!(
            decode_persisted(
                "ENRICHMENT_EMBEDDED",
                "ENRICHMENT",
                1,
                &json!({"sourceEventId": "01900000-0000-7000-8000-000000000001"}),
            ),
            Ok(ProductListingPersistedEvent::Embedded)
        ));
        assert!(matches!(
            decode_persisted(
                "ENRICHMENT_TRANSLATED_TITLES",
                "ENRICHMENT",
                1,
                &json!({
                    "sourceEventId": "01900000-0000-7000-8000-000000000001",
                    "sourceLanguage": "de",
                    "targetLanguages": ["en"]
                }),
            ),
            Ok(ProductListingPersistedEvent::TranslatedTitles)
        ));
    }

    fn canonical_discovery_value() -> Value {
        encode(&discovered_payload()).unwrap_or_else(|error| panic!("encode: {error}"))
    }

    fn discovered_payload() -> ProductListingEventPayload {
        ProductListingEventPayload::rehydrate_discovered(RehydratedProductListingDiscovered {
            listing_source_id: ListingSourceId::try_from(storage_uuid(
                "01900000-0000-7000-8000-000000000001",
            ))
            .unwrap_or_else(|error| panic!("listing source ID: {error}")),
            source_listing_id: SourceListingId::try_from("codec-source-listing")
                .unwrap_or_else(|error| panic!("source ID: {error}")),
            title: Some(Localized::new(Language::En, Title::from("Codec title"))),
            description: Some(Localized::new(
                Language::En,
                Description::from("Codec description"),
            )),
            pricing: ProductListingPricing {
                price: Some(ProductListingPrice::from(price(10))),
                price_estimate_min: None,
                price_estimate_max: None,
            },
            availability: Some(ListingAvailability::Available),
            url: url("https://example.com/old"),
            image_count: ProductListingImageCount::new(1),
            auction: None,
        })
        .unwrap_or_else(|error| panic!("discovery payload: {error}"))
    }

    fn changed_payload(state: RehydratedProductListingChanged) -> ProductListingEventPayload {
        ProductListingEventPayload::rehydrate_changed(state)
            .unwrap_or_else(|error| panic!("changed payload: {error}"))
    }

    fn changed_payload_with_all_dimensions() -> ProductListingEventPayload {
        let fx_rate_id =
            fxrate_core::FxRateId::try_from(storage_uuid("01900000-0000-7000-8000-000000000002"))
                .unwrap_or_else(|error| panic!("FX rate ID: {error}"));
        let observation = ListingSaleObservation::new(OffsetDateTime::UNIX_EPOCH, fx_rate_id);
        changed_payload(RehydratedProductListingChanged {
            price: Some((None, Some(ProductListingPrice::from(price(20))))),
            price_estimate_min: Some((None, Some(price(15)))),
            price_estimate_max: Some((None, Some(price(25)))),
            availability: Some((Some(ListingAvailability::Available), None)),
            url: Some((
                url("https://example.com/old"),
                url("https://example.com/new"),
            )),
            images: Some((
                ProductListingImageCount::new(1),
                ProductListingImageCount::new(2),
            )),
            auction: Some((
                None,
                Some(ProductListingAuction::new(
                    None,
                    Some(
                        LotNumber::try_from("42")
                            .unwrap_or_else(|error| panic!("lot number: {error}")),
                    ),
                    Some(
                        CataloguePosition::new(7)
                            .unwrap_or_else(|error| panic!("catalogue position: {error}")),
                    ),
                    Some(
                        LotAuctionTiming::new(
                            Some(AuctionTime::instant(OffsetDateTime::UNIX_EPOCH, None)),
                            Some(AuctionTime::instant(
                                OffsetDateTime::UNIX_EPOCH + Duration::hours(1),
                                None,
                            )),
                            Some(OffsetDateTime::UNIX_EPOCH + Duration::hours(2)),
                        )
                        .unwrap_or_else(|error| panic!("lot timing: {error}")),
                    ),
                )),
            )),
            lifecycle: Some(ProductListingLifecycleChange::Withdrawn {
                previous_availability: Some(ListingAvailability::Available),
            }),
            sale_observation: Some((None, Some(observation))),
        })
    }

    fn withdrawal_payload() -> ProductListingEventPayload {
        changed_payload(RehydratedProductListingChanged {
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            availability: Some((Some(ListingAvailability::Available), None)),
            url: None,
            images: None,
            auction: None,
            lifecycle: Some(ProductListingLifecycleChange::Withdrawn {
                previous_availability: Some(ListingAvailability::Available),
            }),
            sale_observation: None,
        })
    }

    fn storage_uuid(value: &str) -> uuid::Uuid {
        value
            .parse::<uuid::Uuid>()
            .unwrap_or_else(|error| panic!("storage UUID fixture: {error}"))
    }

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap_or_else(|error| panic!("URL: {error}"))
    }

    fn price(amount: u64) -> Price {
        Price::new(MonetaryAmount::from(amount), Currency::Eur)
    }
}

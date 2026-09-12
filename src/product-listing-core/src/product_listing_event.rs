use crate::description::Description;
use crate::listing_availability::ListingAvailability;
use crate::product_listing::{
    ListingSaleObservation, ProductListingAuction, ProductListingPricing,
};
use crate::product_listing_price::ProductListingPrice;
use crate::source_listing_id::SourceListingId;
use crate::title::Title;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::EnumIter)]
pub enum ProductListingEventType {
    Discovered,
    Changed,
}

impl ProductListingEventType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "PRODUCT_LISTING_DISCOVERED",
            Self::Changed => "PRODUCT_LISTING_CHANGED",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingEventPayload {
    Discovered(ProductListingDiscovered),
    Changed(ProductListingChanged),
}

impl ProductListingEventPayload {
    pub const fn event_type(&self) -> ProductListingEventType {
        match self {
            Self::Discovered(_) => ProductListingEventType::Discovered,
            Self::Changed(_) => ProductListingEventType::Changed,
        }
    }

    /// Rebuilds an immutable discovery payload from validated adapter values.
    #[doc(hidden)]
    pub fn rehydrate_discovered(
        state: RehydratedProductListingDiscovered,
    ) -> Result<Self, RehydrateProductListingEventError> {
        Ok(Self::Discovered(ProductListingDiscovered {
            listing_source_id: state.listing_source_id,
            source_listing_id: state.source_listing_id,
            title: state.title,
            description: state.description,
            pricing: state.pricing,
            availability: state.availability,
            url: state.url,
            image_count: state.image_count,
            auction: state.auction,
        }))
    }

    /// Rebuilds an immutable changed payload from validated adapter values.
    #[doc(hidden)]
    pub fn rehydrate_changed(
        state: RehydratedProductListingChanged,
    ) -> Result<Self, RehydrateProductListingEventError> {
        let changed = ProductListingChanged {
            price: value_change(state.price, "price")?,
            price_estimate_min: value_change(state.price_estimate_min, "price estimate minimum")?,
            price_estimate_max: value_change(state.price_estimate_max, "price estimate maximum")?,
            availability: value_change(state.availability, "availability")?,
            url: value_change(state.url, "URL")?,
            image_count: state
                .images
                .map(|(previous, current)| ProductListingImagesChanged {
                    previous_count: previous,
                    current_count: current,
                }),
            auction: value_change(state.auction, "auction")?.map(Box::new),
            lifecycle: state.lifecycle,
            sale_observation: sale_observation_change(state.sale_observation)?,
        };

        match (&changed.lifecycle, &changed.availability) {
            (Some(ProductListingLifecycleChange::Withdrawn { .. }), Some(availability))
                if availability.current().is_some() =>
            {
                return Err(
                    RehydrateProductListingEventError::WithdrawnEventHasCurrentAvailability,
                );
            }
            (Some(ProductListingLifecycleChange::Restored), Some(availability))
                if availability.previous().is_some() =>
            {
                return Err(
                    RehydrateProductListingEventError::RestoredEventHasPreviousAvailability,
                );
            }
            _ => {}
        }

        if changed.is_empty() {
            return Err(RehydrateProductListingEventError::EmptyChanged);
        }
        Ok(Self::Changed(changed))
    }
}

/// Adapter input for rebuilding an immutable discovery payload.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedProductListingDiscovered {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductListingPricing,
    pub availability: Option<ListingAvailability>,
    pub url: Url,
    pub image_count: ProductListingImageCount,
    pub auction: Option<ProductListingAuction>,
}

/// Adapter inputs for rebuilding an immutable changed payload.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedProductListingChanged {
    pub price: Option<(Option<ProductListingPrice>, Option<ProductListingPrice>)>,
    pub price_estimate_min: Option<(Option<Price>, Option<Price>)>,
    pub price_estimate_max: Option<(Option<Price>, Option<Price>)>,
    pub availability: Option<(Option<ListingAvailability>, Option<ListingAvailability>)>,
    pub url: Option<(Url, Url)>,
    pub images: Option<(ProductListingImageCount, ProductListingImageCount)>,
    pub auction: Option<(Option<ProductListingAuction>, Option<ProductListingAuction>)>,
    pub lifecycle: Option<ProductListingLifecycleChange>,
    pub sale_observation: Option<(
        Option<ListingSaleObservation>,
        Option<ListingSaleObservation>,
    )>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProductListingImageCountConversionError {
    #[error("product listing image count exceeds u64")]
    Overflow,
}

/// Fixed-width count used by persisted and public ProductListing event contracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProductListingImageCount(u64);

impl ProductListingImageCount {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

impl TryFrom<usize> for ProductListingImageCount {
    type Error = ProductListingImageCountConversionError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map(Self)
            .map_err(|_| ProductListingImageCountConversionError::Overflow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RehydrateProductListingEventError {
    #[error("changed ProductListing event payload is empty")]
    EmptyChanged,
    #[error("ProductListing event {field} has equal previous and current values")]
    EqualValues { field: &'static str },

    #[error("withdrawal event has current availability")]
    WithdrawnEventHasCurrentAvailability,
    #[error("restoration event has previous availability")]
    RestoredEventHasPreviousAvailability,
    #[error("ProductListing sale observation correction is unsupported")]
    SaleObservationCorrectionUnsupported,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingDiscovered {
    listing_source_id: ListingSourceId,
    source_listing_id: SourceListingId,
    title: Option<Localized<Language, Title>>,
    description: Option<Localized<Language, Description>>,
    pricing: ProductListingPricing,
    availability: Option<ListingAvailability>,
    url: Url,
    image_count: ProductListingImageCount,
    auction: Option<ProductListingAuction>,
}

impl ProductListingDiscovered {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        listing_source_id: ListingSourceId,
        source_listing_id: SourceListingId,
        title: Option<Localized<Language, Title>>,
        description: Option<Localized<Language, Description>>,
        pricing: ProductListingPricing,
        availability: Option<ListingAvailability>,
        url: Url,
        image_count: ProductListingImageCount,
        auction: Option<ProductListingAuction>,
    ) -> Self {
        Self {
            listing_source_id,
            source_listing_id,
            title,
            description,
            pricing,
            availability,
            url,
            image_count,
            auction,
        }
    }

    pub const fn listing_source_id(&self) -> ListingSourceId {
        self.listing_source_id
    }

    pub fn source_listing_id(&self) -> &SourceListingId {
        &self.source_listing_id
    }

    pub fn title(&self) -> Option<&Localized<Language, Title>> {
        self.title.as_ref()
    }

    pub fn description(&self) -> Option<&Localized<Language, Description>> {
        self.description.as_ref()
    }

    pub const fn pricing(&self) -> ProductListingPricing {
        self.pricing
    }

    pub const fn availability(&self) -> Option<ListingAvailability> {
        self.availability
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub const fn image_count(&self) -> ProductListingImageCount {
        self.image_count
    }

    pub fn auction(&self) -> Option<&ProductListingAuction> {
        self.auction.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValueChange<T> {
    previous: T,
    current: T,
}

impl<T> ValueChange<T> {
    pub(crate) const fn new(previous: T, current: T) -> Self {
        Self { previous, current }
    }

    pub const fn previous(&self) -> &T {
        &self.previous
    }

    pub const fn current(&self) -> &T {
        &self.current
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingImagesChanged {
    previous_count: ProductListingImageCount,
    current_count: ProductListingImageCount,
}

impl ProductListingImagesChanged {
    pub const fn previous_count(&self) -> ProductListingImageCount {
        self.previous_count
    }

    pub const fn current_count(&self) -> ProductListingImageCount {
        self.current_count
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductListingLifecycleChange {
    Withdrawn {
        previous_availability: Option<ListingAvailability>,
    },
    Restored,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingChanged {
    price: Option<ValueChange<Option<ProductListingPrice>>>,
    price_estimate_min: Option<ValueChange<Option<Price>>>,
    price_estimate_max: Option<ValueChange<Option<Price>>>,
    availability: Option<ValueChange<Option<ListingAvailability>>>,
    url: Option<ValueChange<Url>>,
    image_count: Option<ProductListingImagesChanged>,
    auction: Option<Box<ValueChange<Option<ProductListingAuction>>>>,
    lifecycle: Option<ProductListingLifecycleChange>,
    sale_observation: Option<ValueChange<Option<ListingSaleObservation>>>,
}

impl ProductListingChanged {
    pub(crate) const fn empty() -> Self {
        Self {
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            availability: None,
            url: None,
            image_count: None,
            auction: None,
            lifecycle: None,
            sale_observation: None,
        }
    }

    pub(crate) fn change_price(
        &mut self,
        previous: Option<ProductListingPrice>,
        current: Option<ProductListingPrice>,
    ) {
        coalesce_value_change(&mut self.price, previous, current);
    }

    pub(crate) fn change_price_estimate_min(
        &mut self,
        previous: Option<Price>,
        current: Option<Price>,
    ) {
        coalesce_value_change(&mut self.price_estimate_min, previous, current);
    }

    pub(crate) fn change_price_estimate_max(
        &mut self,
        previous: Option<Price>,
        current: Option<Price>,
    ) {
        coalesce_value_change(&mut self.price_estimate_max, previous, current);
    }

    pub(crate) fn change_availability(
        &mut self,
        previous: Option<ListingAvailability>,
        current: Option<ListingAvailability>,
    ) {
        coalesce_value_change(&mut self.availability, previous, current);
    }

    pub(crate) fn change_url(&mut self, previous: Url, current: Url) {
        coalesce_value_change(&mut self.url, previous, current);
    }

    pub(crate) fn set_image_count_change(
        &mut self,
        previous: ProductListingImageCount,
        current: ProductListingImageCount,
    ) {
        self.image_count = Some(ProductListingImagesChanged {
            previous_count: previous,
            current_count: current,
        });
    }

    pub(crate) fn clear_image_count(&mut self) {
        self.image_count = None;
    }

    pub(crate) fn change_auction(
        &mut self,
        previous: Option<ProductListingAuction>,
        current: Option<ProductListingAuction>,
    ) {
        let mut auction = self.auction.take().map(|change| *change);
        coalesce_value_change(&mut auction, previous, current);
        self.auction = auction.map(Box::new);
    }

    pub(crate) fn change_lifecycle(&mut self, change: ProductListingLifecycleChange) {
        self.lifecycle = match (&self.lifecycle, change) {
            (
                Some(ProductListingLifecycleChange::Withdrawn { .. }),
                ProductListingLifecycleChange::Restored,
            )
            | (
                Some(ProductListingLifecycleChange::Restored),
                ProductListingLifecycleChange::Withdrawn { .. },
            ) => None,
            (_, change) => Some(change),
        };
    }

    pub(crate) fn validate_sale_observation_change(
        &self,
        previous: Option<ListingSaleObservation>,
        current: Option<ListingSaleObservation>,
    ) -> Result<(), ProductListingSaleObservationChangeError> {
        let first_previous = self
            .sale_observation
            .as_ref()
            .map_or(previous, |change| *change.previous());
        if let (Some(previous), Some(current)) = (first_previous, current)
            && previous != current
        {
            return Err(ProductListingSaleObservationChangeError::CorrectionUnsupported);
        }
        Ok(())
    }

    pub(crate) fn change_sale_observation(
        &mut self,
        previous: Option<ListingSaleObservation>,
        current: Option<ListingSaleObservation>,
    ) {
        let first_previous = self
            .sale_observation
            .as_ref()
            .map_or(previous, |change| *change.previous());
        self.sale_observation =
            (first_previous != current).then_some(ValueChange::new(first_previous, current));
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.price.is_none()
            && self.price_estimate_min.is_none()
            && self.price_estimate_max.is_none()
            && self.availability.is_none()
            && self.url.is_none()
            && self.image_count.is_none()
            && self.auction.is_none()
            && self.lifecycle.is_none()
            && self.sale_observation.is_none()
    }

    pub const fn price(&self) -> Option<&ValueChange<Option<ProductListingPrice>>> {
        self.price.as_ref()
    }

    pub const fn price_estimate_min(&self) -> Option<&ValueChange<Option<Price>>> {
        self.price_estimate_min.as_ref()
    }

    pub const fn price_estimate_max(&self) -> Option<&ValueChange<Option<Price>>> {
        self.price_estimate_max.as_ref()
    }

    pub const fn availability(&self) -> Option<&ValueChange<Option<ListingAvailability>>> {
        self.availability.as_ref()
    }

    pub const fn url(&self) -> Option<&ValueChange<Url>> {
        self.url.as_ref()
    }

    pub const fn image_count(&self) -> Option<&ProductListingImagesChanged> {
        self.image_count.as_ref()
    }

    pub fn auction(&self) -> Option<&ValueChange<Option<ProductListingAuction>>> {
        self.auction.as_deref()
    }

    pub const fn lifecycle(&self) -> Option<&ProductListingLifecycleChange> {
        self.lifecycle.as_ref()
    }

    pub const fn sale_observation(&self) -> Option<&ValueChange<Option<ListingSaleObservation>>> {
        self.sale_observation.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ProductListingSaleObservationChangeError {
    #[error("a sale observation correction is unsupported")]
    CorrectionUnsupported,
}

fn value_change<T: PartialEq>(
    change: Option<(T, T)>,
    field: &'static str,
) -> Result<Option<ValueChange<T>>, RehydrateProductListingEventError> {
    let Some((previous, current)) = change else {
        return Ok(None);
    };
    if previous == current {
        return Err(RehydrateProductListingEventError::EqualValues { field });
    }
    Ok(Some(ValueChange::new(previous, current)))
}

fn sale_observation_change(
    change: Option<(
        Option<ListingSaleObservation>,
        Option<ListingSaleObservation>,
    )>,
) -> Result<Option<ValueChange<Option<ListingSaleObservation>>>, RehydrateProductListingEventError>
{
    let Some((previous, current)) = change else {
        return Ok(None);
    };
    if previous == current {
        return Err(RehydrateProductListingEventError::EqualValues {
            field: "sale observation",
        });
    }
    if previous.is_some() && current.is_some() {
        return Err(RehydrateProductListingEventError::SaleObservationCorrectionUnsupported);
    }
    Ok(Some(ValueChange::new(previous, current)))
}

fn coalesce_value_change<T: Clone + PartialEq>(
    change: &mut Option<ValueChange<T>>,
    previous: T,
    current: T,
) {
    let first_previous = change
        .as_ref()
        .map_or(previous, |existing| existing.previous.clone());

    if first_previous == current {
        *change = None;
        return;
    }

    *change = Some(ValueChange::new(first_previous, current));
}

#[cfg(test)]
mod tests {
    use super::*;
    use money::{Currency, MonetaryAmount};
    use std::collections::HashSet;
    use strum::IntoEnumIterator;

    #[test]
    fn should_use_only_unique_canonical_product_listing_event_types() {
        let event_types = ProductListingEventType::iter()
            .map(ProductListingEventType::as_str)
            .collect::<HashSet<_>>();

        assert_eq!(2, event_types.len());
        assert!(event_types.contains("PRODUCT_LISTING_DISCOVERED"));
        assert!(event_types.contains("PRODUCT_LISTING_CHANGED"));
    }

    #[test]
    fn should_keep_image_change_when_counts_are_equal() {
        let mut changed = ProductListingChanged::empty();
        let count = ProductListingImageCount::new(2);

        changed.set_image_count_change(count, count);

        assert_eq!(
            Some(count),
            changed.image_count().map(|value| value.previous_count())
        );
        assert_eq!(
            Some(count),
            changed.image_count().map(|value| value.current_count())
        );
        assert!(!changed.is_empty());
    }

    #[test]
    fn should_cancel_sale_observation_transitions_in_both_directions() {
        let observation = ListingSaleObservation::new(
            time::OffsetDateTime::UNIX_EPOCH,
            fxrate_core::FxRateId::new(),
        );
        let mut changed = ProductListingChanged::empty();
        changed.change_sale_observation(None, Some(observation));
        changed.change_sale_observation(Some(observation), None);
        assert!(changed.sale_observation().is_none());

        changed.change_sale_observation(Some(observation), None);
        changed.change_sale_observation(None, Some(observation));
        assert!(changed.sale_observation().is_none());
    }

    #[test]
    fn should_reject_sale_observation_correction() {
        let first = ListingSaleObservation::new(
            time::OffsetDateTime::UNIX_EPOCH,
            fxrate_core::FxRateId::new(),
        );
        let second = ListingSaleObservation::new(
            time::OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1),
            fxrate_core::FxRateId::new(),
        );
        let mut changed = ProductListingChanged::empty();
        changed.change_sale_observation(Some(first), None);

        assert_eq!(
            Err(ProductListingSaleObservationChangeError::CorrectionUnsupported),
            changed.validate_sale_observation_change(None, Some(second))
        );
        assert_eq!(
            Err(RehydrateProductListingEventError::SaleObservationCorrectionUnsupported),
            ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                availability: None,
                url: None,
                images: None,
                auction: None,
                lifecycle: None,
                sale_observation: Some((Some(first), Some(second))),
            })
        );
    }

    #[test]
    fn should_reject_equal_ordinary_rehydrated_change() {
        let price = Price::new(MonetaryAmount::from(100_u64), Currency::Eur);

        assert_eq!(
            Err(RehydrateProductListingEventError::EqualValues { field: "price" }),
            ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
                price: Some((
                    Some(ProductListingPrice::from(price)),
                    Some(ProductListingPrice::from(price)),
                )),
                price_estimate_min: None,
                price_estimate_max: None,
                availability: None,
                url: None,
                images: None,
                auction: None,
                lifecycle: None,
                sale_observation: None,
            })
        );
    }

    #[test]
    fn should_rehydrate_on_request_price_change() {
        let payload =
            ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
                price: Some((Some(ProductListingPrice::OnRequest), None)),
                price_estimate_min: None,
                price_estimate_max: None,
                availability: None,
                url: None,
                images: None,
                auction: None,
                lifecycle: None,
                sale_observation: None,
            });

        assert!(matches!(
            payload,
            Ok(ProductListingEventPayload::Changed(changed))
                if changed.price().is_some_and(|price| {
                    *price.previous() == Some(ProductListingPrice::OnRequest)
                        && price.current().is_none()
                })
        ));
    }

    #[test]
    fn should_reject_lifecycle_availability_composites_that_violate_final_state() {
        let withdrawn =
            ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                availability: Some((None, Some(ListingAvailability::Available))),
                url: None,
                images: None,
                auction: None,
                lifecycle: Some(ProductListingLifecycleChange::Withdrawn {
                    previous_availability: None,
                }),
                sale_observation: None,
            });
        assert_eq!(
            Err(RehydrateProductListingEventError::WithdrawnEventHasCurrentAvailability),
            withdrawn
        );

        let restored =
            ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                availability: Some((Some(ListingAvailability::Available), None)),
                url: None,
                images: None,
                auction: None,
                lifecycle: Some(ProductListingLifecycleChange::Restored),
                sale_observation: None,
            });
        assert_eq!(
            Err(RehydrateProductListingEventError::RestoredEventHasPreviousAvailability),
            restored
        );
    }

    #[test]
    fn should_accept_valid_lifecycle_availability_composites() {
        for (availability, lifecycle) in [
            (
                Some((Some(ListingAvailability::Available), None)),
                ProductListingLifecycleChange::Withdrawn {
                    previous_availability: Some(ListingAvailability::Available),
                },
            ),
            (
                Some((None, Some(ListingAvailability::Available))),
                ProductListingLifecycleChange::Restored,
            ),
        ] {
            assert!(
                ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    availability,
                    url: None,
                    images: None,
                    auction: None,
                    lifecycle: Some(lifecycle),
                    sale_observation: None,
                })
                .is_ok()
            );
        }
    }

    #[test]
    fn should_reject_empty_rehydrated_changed_payload() {
        let result =
            ProductListingEventPayload::rehydrate_changed(RehydratedProductListingChanged {
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                availability: None,
                url: None,
                images: None,
                auction: None,
                lifecycle: None,
                sale_observation: None,
            });

        assert_eq!(Err(RehydrateProductListingEventError::EmptyChanged), result);
    }
}

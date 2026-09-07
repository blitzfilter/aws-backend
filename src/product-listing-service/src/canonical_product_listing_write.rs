use crate::ports::{
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingWriteEffects,
    stamp_product_listing_event,
};
use crate::product_listing_title_slug_creation::{
    ProductListingTitleSlugGenerator, RandomProductListingTitleSlugGenerator,
};
use application::error::{BoxError, box_error};
use application::patch_field::PatchField;
use domain_primitives::change_outcome::ChangeOutcome;
use domain_primitives::event_id::EventId;
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    NewProductListing, ProductListing, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use time::OffsetDateTime;
use url::Url;

/// Internal canonical write intent for caller-owned transactions.
///
/// It is not an inbound use case. `product-service` uses it to retain ProductListing
/// aggregate behavior and event semantics while owning the surrounding raw-progress transaction.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalProductListingUpsert {
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: SourceListingId,
    pub title: PatchField<Localized<Language, Title>>,
    pub description: PatchField<Localized<Language, Description>>,
    pub price: PatchField<Price>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
    pub auction_start: PatchField<OffsetDateTime>,
    pub auction_end: PatchField<OffsetDateTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalProductListingWriteResult {
    pub product_listing_id: ProductListingId,
    pub product_listing_event_id: Option<EventId>,
    pub outcome: ChangeOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum CanonicalProductListingWriteError {
    #[error("bound product listing was not found")]
    BoundProductListingNotFound,
    #[error("bound product listing identity does not match raw stream identity")]
    BoundProductListingIdentityMismatch,
    #[error("canonical product listing input is invalid")]
    InvalidInput {
        #[source]
        source: BoxError,
    },
    #[error("canonical product listing persistence failed")]
    Persistence {
        #[source]
        source: BoxError,
    },
    #[error("canonical product listing event append failed")]
    EventAppend {
        #[source]
        source: BoxError,
    },
}

pub struct CanonicalProductListingWriter;

impl CanonicalProductListingWriter {
    pub async fn upsert_in_transaction<Tx, R, E>(
        tx: &mut Tx,
        products: &R,
        events: &E,
        bound_product_listing_id: Option<ProductListingId>,
        command: CanonicalProductListingUpsert,
    ) -> Result<CanonicalProductListingWriteResult, CanonicalProductListingWriteError>
    where
        R: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
    {
        let existing = match bound_product_listing_id {
            Some(product_listing_id) => products
                .in_transaction(tx)
                .find_by_id(product_listing_id)
                .await
                .map_err(map_repository_error)?
                .ok_or(CanonicalProductListingWriteError::BoundProductListingNotFound)?,
            None => {
                let key = product_listing_core::product_listing_id::ProductListingKey::new(
                    command.listing_source_id,
                    command.source_listing_id.clone(),
                );
                let existing = products
                    .in_transaction(tx)
                    .find_by_key(&key)
                    .await
                    .map_err(map_repository_error)?;
                let Some(existing) = existing else {
                    return Self::create(tx, products, events, command).await;
                };
                existing
            }
        };

        let expected_version = existing.version;
        let mut product = existing.value;
        if product.listing_source_id() != command.listing_source_id
            || product.source_listing_id() != &command.source_listing_id
        {
            return Err(CanonicalProductListingWriteError::BoundProductListingIdentityMismatch);
        }
        product
            .restore()
            .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(error),
            })?;
        apply_update(&mut product, &command)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
        });
        let event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            products
                .in_transaction(tx)
                .update(&product, expected_version, event.event_id, effects)
                .await
                .map_err(map_repository_error)?;
            events
                .in_transaction(tx)
                .append(&event)
                .await
                .map_err(|error| CanonicalProductListingWriteError::EventAppend {
                    source: box_error(error),
                })?;
        }
        Ok(CanonicalProductListingWriteResult {
            product_listing_id: product.id(),
            product_listing_event_id: event_id,
            outcome: if event_id.is_some() {
                ChangeOutcome::Changed
            } else {
                ChangeOutcome::Unchanged
            },
        })
    }

    pub async fn withdraw_in_transaction<Tx, R, E>(
        tx: &mut Tx,
        products: &R,
        events: &E,
        product_listing_id: ProductListingId,
    ) -> Result<CanonicalProductListingWriteResult, CanonicalProductListingWriteError>
    where
        R: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
    {
        let loaded = products
            .in_transaction(tx)
            .find_by_id(product_listing_id)
            .await
            .map_err(map_repository_error)?
            .ok_or(CanonicalProductListingWriteError::BoundProductListingNotFound)?;
        let expected_version = loaded.version;
        let mut product = loaded.value;
        product.withdraw().map_err(invalid_input)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
        });
        let event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            products
                .in_transaction(tx)
                .update(&product, expected_version, event.event_id, effects)
                .await
                .map_err(map_repository_error)?;
            events
                .in_transaction(tx)
                .append(&event)
                .await
                .map_err(|error| CanonicalProductListingWriteError::EventAppend {
                    source: box_error(error),
                })?;
        }
        Ok(CanonicalProductListingWriteResult {
            product_listing_id,
            product_listing_event_id: event_id,
            outcome: if event_id.is_some() {
                ChangeOutcome::Changed
            } else {
                ChangeOutcome::Unchanged
            },
        })
    }

    async fn create<Tx, R, E>(
        tx: &mut Tx,
        products: &R,
        events: &E,
        command: CanonicalProductListingUpsert,
    ) -> Result<CanonicalProductListingWriteResult, CanonicalProductListingWriteError>
    where
        R: ProductListingRepositoryFactory<Tx>,
        E: ProductListingEventAppenderFactory<Tx>,
    {
        let title = patch_value(command.title);
        let slug_title = title
            .as_ref()
            .map_or("listing", |value| value.payload.as_ref());
        let title_slug_id = RandomProductListingTitleSlugGenerator
            .generate(slug_title)
            .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(error),
            })?;
        let url = match command.url {
            PatchField::Set(url) => url,
            PatchField::Unchanged | PatchField::Clear => {
                return Err(CanonicalProductListingWriteError::InvalidInput {
                    source: box_error(std::io::Error::other("new raw listing requires URL")),
                });
            }
        };
        let mut product = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id,
            listing_source_id: command.listing_source_id,
            source_listing_id: command.source_listing_id,
            title,
            description: patch_value(command.description),
            pricing: ProductListingPricing {
                price: patch_value(command.price),
                price_estimate_min: patch_value(command.price_estimate_min),
                price_estimate_max: patch_value(command.price_estimate_max),
            },
            availability: patch_value(command.availability),
            url,
            images: match command.images {
                PatchField::Set(images) => images,
                PatchField::Unchanged | PatchField::Clear => IndexSet::new(),
            },
            auction: ProductListingAuction {
                start: patch_value(command.auction_start),
                end: patch_value(command.auction_end),
            },
        })
        .map_err(|error| CanonicalProductListingWriteError::InvalidInput {
            source: box_error(error),
        })?;
        let event = product
            .take_pending_event_payload()
            .map(|payload| {
                stamp_product_listing_event(product.id(), OffsetDateTime::now_utc(), payload)
            })
            .ok_or_else(|| CanonicalProductListingWriteError::InvalidInput {
                source: box_error(std::io::Error::other(
                    "new listing did not produce discovery event",
                )),
            })?;
        products
            .in_transaction(tx)
            .insert(&product, event.event_id)
            .await
            .map_err(map_repository_error)?;
        events
            .in_transaction(tx)
            .append(&event)
            .await
            .map_err(|error| CanonicalProductListingWriteError::EventAppend {
                source: box_error(error),
            })?;
        Ok(CanonicalProductListingWriteResult {
            product_listing_id: product.id(),
            product_listing_event_id: Some(event.event_id),
            outcome: ChangeOutcome::Changed,
        })
    }
}

fn patch_value<T>(patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Set(value) => Some(value),
        PatchField::Unchanged | PatchField::Clear => None,
    }
}

fn apply_update(
    product: &mut ProductListing,
    command: &CanonicalProductListingUpsert,
) -> Result<(), CanonicalProductListingWriteError> {
    let mut pricing = product.pricing();
    apply_option_patch(&mut pricing.price, command.price.clone());
    apply_option_patch(
        &mut pricing.price_estimate_min,
        command.price_estimate_min.clone(),
    );
    apply_option_patch(
        &mut pricing.price_estimate_max,
        command.price_estimate_max.clone(),
    );
    product.replace_pricing(pricing).map_err(invalid_input)?;

    match command.availability {
        PatchField::Set(availability) => {
            product
                .set_availability(availability)
                .map_err(invalid_input)?;
        }
        PatchField::Clear => {
            product.clear_availability().map_err(invalid_input)?;
        }
        PatchField::Unchanged => {}
    }
    if let PatchField::Set(url) = &command.url {
        product.change_url(url.clone()).map_err(invalid_input)?;
    }
    match &command.images {
        PatchField::Set(images) => {
            product
                .replace_images(images.clone())
                .map_err(invalid_input)?;
        }
        PatchField::Clear => {
            product
                .replace_images(IndexSet::new())
                .map_err(invalid_input)?;
        }
        PatchField::Unchanged => {}
    };
    let mut auction = product.auction();
    apply_option_patch(&mut auction.start, command.auction_start.clone());
    apply_option_patch(&mut auction.end, command.auction_end.clone());
    product.replace_auction(auction).map_err(invalid_input)?;
    Ok(())
}

fn apply_option_patch<T>(target: &mut Option<T>, patch: PatchField<T>) {
    match patch {
        PatchField::Set(value) => *target = Some(value),
        PatchField::Clear => *target = None,
        PatchField::Unchanged => {}
    }
}

fn invalid_input(
    error: impl std::error::Error + Send + Sync + 'static,
) -> CanonicalProductListingWriteError {
    CanonicalProductListingWriteError::InvalidInput {
        source: box_error(error),
    }
}

fn map_repository_error(error: ProductListingRepositoryError) -> CanonicalProductListingWriteError {
    CanonicalProductListingWriteError::Persistence {
        source: box_error(error),
    }
}

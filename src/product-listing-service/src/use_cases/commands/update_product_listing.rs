use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventAppendError,
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, ProductListingWriteEffects,
    stamp_product_listing_event,
};
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::change_outcome::ChangeOutcome;
use indexmap::IndexSet;
use money::Price;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    ChangeListingAvailabilityError, ChangeProductListingError, ProductListing,
};
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::product_listing_price::ProductListingPrice;
use url::Url;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateProductListingCommand {
    pub price: PatchField<ProductListingPrice>,
    pub price_estimate_min: PatchField<Price>,
    pub price_estimate_max: PatchField<Price>,
    pub availability: PatchField<ListingAvailability>,
    pub url: PatchField<Url>,
    pub images: PatchField<IndexSet<ProductListingImage>>,
    pub auction_start: PatchField<Option<time::OffsetDateTime>>,
    pub auction_end: PatchField<Option<time::OffsetDateTime>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateProductListingResult {
    pub product_listing_id: ProductListingId,
    pub outcome: ChangeOutcome,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateProductListingError {
    #[error("authenticated actor required to update product listing")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    ListingSourceNotFound,
    #[error("partner product listing authorization is temporarily unavailable")]
    PartnerAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product listing authorization failed internally")]
    PartnerAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("product listing not found")]
    NotFound,
    #[error("product listing is withdrawn")]
    ListingWithdrawn,
    #[error("product listing URL is required")]
    UrlRequired,
    #[error("product listing is invalid")]
    InvalidProductListing,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventAppenderFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin update product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update product listing transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError>;
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError>;
}

pub struct UpdateProductListingHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}
impl<U, R, E, A> UpdateProductListingHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}
enum UpdateTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A> UpdateProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    async fn update(
        &self,
        context: &OperationContext,
        target: UpdateTarget,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<UpdateProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        if matches!(command.url, PatchField::Clear) {
            return Err(UpdateProductListingError::UrlRequired);
        }
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateProductListingError::BeginTransactionFailed)?;
        let loaded = match target {
            UpdateTarget::Id(id) => {
                let loaded = self
                    .products
                    .in_transaction(&mut tx)
                    .find_by_id(id)
                    .await?
                    .ok_or(UpdateProductListingError::NotFound)?;
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, loaded.value.listing_source_id())
                        .await?;
                }
                loaded
            }
            UpdateTarget::Key(key) => {
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, key.listing_source_id)
                        .await?;
                }
                self.products
                    .in_transaction(&mut tx)
                    .find_by_key(&key)
                    .await?
                    .ok_or(UpdateProductListingError::NotFound)?
            }
        };
        let expected_version = loaded.version;
        let mut product = loaded.value;
        apply_command(&mut product, command)?;
        let event = product.take_pending_event_payload().map(|payload| {
            stamp_product_listing_event(product.id(), time::OffsetDateTime::now_utc(), payload)
        });
        let outcome = if event.is_some() {
            ChangeOutcome::Changed
        } else {
            ChangeOutcome::Unchanged
        };
        let current_event_id = event.as_ref().map(|event| event.event_id);
        if let Some(event) = event {
            let effects = ProductListingWriteEffects::from(&event.payload);
            product = self
                .products
                .in_transaction(&mut tx)
                .update(&product, expected_version, event.event_id, effects)
                .await?
                .value;
            self.events.in_transaction(&mut tx).append(&event).await?;
        }
        tx.commit()
            .await
            .map_err(|_| UpdateProductListingError::CommitTransactionFailed)?;
        tracing::info!(event = "product_listing.updated", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %product.id(), event_id = ?current_event_id, outcome = "success");
        Ok(UpdateProductListingResult {
            product_listing_id: product.id(),
            outcome,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> UpdateProductListingUseCase for UpdateProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(name = "update_product_listing", skip_all, fields(product_listing_id = %product_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        self.update(context, UpdateTarget::Id(product_listing_id), command)
            .await
    }
    #[tracing::instrument(name = "update_product_listing_by_key", skip_all, fields(listing_source_id = %product_key.listing_source_id, source_listing_id = %product_key.source_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
        command: UpdateProductListingCommand,
    ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
        self.update(context, UpdateTarget::Key(product_key), command)
            .await
    }
}

fn apply_command(
    product: &mut ProductListing,
    command: UpdateProductListingCommand,
) -> Result<(), UpdateProductListingError> {
    let mut pricing = product.pricing();
    let price_changed = apply_price_patch(&mut pricing.price, command.price);
    let price_estimate_min_changed =
        apply_price_patch(&mut pricing.price_estimate_min, command.price_estimate_min);
    let price_estimate_max_changed =
        apply_price_patch(&mut pricing.price_estimate_max, command.price_estimate_max);
    if price_changed || price_estimate_min_changed || price_estimate_max_changed {
        product.replace_pricing(pricing)?;
    }
    match command.availability {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.set_availability(value)?;
        }
        PatchField::Clear => {
            product.clear_availability()?;
        }
    }
    match command.url {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.change_url(value)?;
        }
        PatchField::Clear => {
            return Err(UpdateProductListingError::UrlRequired);
        }
    }
    match command.images {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            product.replace_images(value)?;
        }
        PatchField::Clear => {
            product.replace_images(Default::default())?;
        }
    }
    let mut auction = product.auction();
    let auction_start_changed = apply_auction_patch(&mut auction.start, command.auction_start);
    let auction_end_changed = apply_auction_patch(&mut auction.end, command.auction_end);
    if auction_start_changed || auction_end_changed {
        product.replace_auction(auction)?;
    }
    Ok(())
}

fn apply_price_patch<T>(field: &mut Option<T>, patch: PatchField<T>) -> bool {
    match patch {
        PatchField::Unchanged => false,
        PatchField::Set(value) => {
            *field = Some(value);
            true
        }
        PatchField::Clear => {
            *field = None;
            true
        }
    }
}

fn apply_auction_patch(
    field: &mut Option<time::OffsetDateTime>,
    patch: PatchField<Option<time::OffsetDateTime>>,
) -> bool {
    match patch {
        PatchField::Unchanged => false,
        PatchField::Set(value) => {
            *field = value;
            true
        }
        PatchField::Clear => {
            *field = None;
            true
        }
    }
}
fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Some(*id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<ChangeListingAvailabilityError> for UpdateProductListingError {
    fn from(_: ChangeListingAvailabilityError) -> Self {
        Self::ListingWithdrawn
    }
}
impl From<ChangeProductListingError> for UpdateProductListingError {
    fn from(error: ChangeProductListingError) -> Self {
        match error {
            ChangeProductListingError::ListingWithdrawn => Self::ListingWithdrawn,
            ChangeProductListingError::AuctionStartAfterEnd
            | ChangeProductListingError::ImageCountOverflow
            | ChangeProductListingError::InitialDiscoveryLifecycleChange
            | ChangeProductListingError::ConflictingPendingObservation => {
                Self::InvalidProductListing
            }
        }
    }
}
impl From<OperationAuthorizationError> for UpdateProductListingError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}
impl From<PartnerProductListingAuthorizationError> for UpdateProductListingError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        match error {
            PartnerProductListingAuthorizationError::ListingSourceNotFound => {
                Self::ListingSourceNotFound
            }
            PartnerProductListingAuthorizationError::Forbidden => Self::Forbidden,
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerAuthorizationInternal { source }
            }
        }
    }
}
impl From<ProductListingRepositoryError> for UpdateProductListingError {
    fn from(_: ProductListingRepositoryError) -> Self {
        Self::PersistenceFailed
    }
}
impl From<ProductListingEventAppendError> for UpdateProductListingError {
    fn from(error: ProductListingEventAppendError) -> Self {
        Self::EventAppenderFailed {
            source: box_error(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::ListingSourceId;
    use localization::{Language, Localized};
    use money::{Currency, MonetaryAmount};
    use product_listing_core::{
        product_listing::{NewProductListing, ProductListingAuction, ProductListingPricing},
        product_listing_event::ProductListingEventPayload,
        product_listing_id::ProductListingId,
        product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
        title::Title,
    };

    fn price(amount: u64) -> Price {
        Price::new(MonetaryAmount::from(amount), Currency::Eur)
    }

    #[test]
    fn should_reject_clearing_required_url_without_mutating_listing() {
        let url = Url::parse("https://shop.example/listing")
            .unwrap_or_else(|error| panic!("invalid URL: {error}"));
        let mut listing = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("listing-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: Some(Localized::new(Language::En, Title::from("Listing"))),
            description: None,
            pricing: ProductListingPricing::default(),
            availability: None,
            url: url.clone(),
            images: IndexSet::new(),
            auction: ProductListingAuction::default(),
        })
        .unwrap_or_else(|error| panic!("valid listing should be created: {error}"));

        let result = apply_command(
            &mut listing,
            UpdateProductListingCommand {
                url: PatchField::Clear,
                ..Default::default()
            },
        );

        assert!(matches!(
            result,
            Err(UpdateProductListingError::UrlRequired)
        ));
        assert_eq!(&url, listing.url());
    }

    #[test]
    fn should_emit_one_price_event_with_final_pricing_for_combined_leaf_patches() {
        let old_pricing = ProductListingPricing {
            price: Some(ProductListingPrice::from(price(100))),
            price_estimate_min: Some(price(110)),
            price_estimate_max: Some(price(120)),
        };
        let new_pricing = ProductListingPricing {
            price: Some(ProductListingPrice::from(price(200))),
            price_estimate_min: Some(price(210)),
            price_estimate_max: Some(price(220)),
        };
        let mut listing = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("listing-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            pricing: old_pricing,
            availability: None,
            url: Url::parse("https://shop.example/listing")
                .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            images: IndexSet::new(),
            auction: ProductListingAuction::default(),
        })
        .unwrap_or_else(|error| panic!("valid listing should be created: {error}"));
        listing.take_pending_event_payload();

        apply_command(
            &mut listing,
            UpdateProductListingCommand {
                price: PatchField::Set(ProductListingPrice::from(price(200))),
                price_estimate_min: PatchField::Set(price(210)),
                price_estimate_max: PatchField::Set(price(220)),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("valid price update: {error}"));

        assert_eq!(listing.pricing(), new_pricing);
        let Some(ProductListingEventPayload::Changed(change)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected changed payload");
        };
        assert_eq!(
            Some(&old_pricing.price),
            change.price().map(|value| value.previous())
        );
        assert_eq!(
            Some(&new_pricing.price),
            change.price().map(|value| value.current())
        );
        assert_eq!(
            Some(&old_pricing.price_estimate_min),
            change.price_estimate_min().map(|value| value.previous())
        );
        assert_eq!(
            Some(&new_pricing.price_estimate_min),
            change.price_estimate_min().map(|value| value.current())
        );
        assert_eq!(
            Some(&old_pricing.price_estimate_max),
            change.price_estimate_max().map(|value| value.previous())
        );
        assert_eq!(
            Some(&new_pricing.price_estimate_max),
            change.price_estimate_max().map(|value| value.current())
        );
    }

    #[test]
    fn should_emit_one_auction_event_with_final_auction_for_combined_leaf_patches() {
        let old_auction = ProductListingAuction {
            start: Some(time::macros::datetime!(2026-01-01 0:00 UTC)),
            end: Some(time::macros::datetime!(2026-01-02 0:00 UTC)),
        };
        let new_auction = ProductListingAuction {
            start: Some(time::macros::datetime!(2026-02-01 0:00 UTC)),
            end: Some(time::macros::datetime!(2026-02-02 0:00 UTC)),
        };
        let mut listing = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("listing-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            pricing: ProductListingPricing::default(),
            availability: None,
            url: Url::parse("https://shop.example/listing")
                .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            images: IndexSet::new(),
            auction: old_auction,
        })
        .unwrap_or_else(|error| panic!("valid listing should be created: {error}"));
        listing.take_pending_event_payload();

        apply_command(
            &mut listing,
            UpdateProductListingCommand {
                auction_start: PatchField::Set(new_auction.start),
                auction_end: PatchField::Set(new_auction.end),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("valid auction update: {error}"));

        assert_eq!(listing.auction(), new_auction);
        let Some(ProductListingEventPayload::Changed(change)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected changed payload");
        };
        assert_eq!(
            Some(&old_auction),
            change.auction().map(|value| value.previous())
        );
        assert_eq!(
            Some(&new_auction),
            change.auction().map(|value| value.current())
        );
    }

    #[test]
    fn should_retain_state_and_pending_events_when_final_auction_is_invalid() {
        let auction = ProductListingAuction {
            start: Some(time::macros::datetime!(2026-01-01 0:00 UTC)),
            end: Some(time::macros::datetime!(2026-01-02 0:00 UTC)),
        };
        let mut listing = ProductListing::create(NewProductListing {
            id: ProductListingId::new(),
            title_slug_id: ProductListingSlugId::raw("listing-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("listing")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: None,
            description: None,
            pricing: ProductListingPricing::default(),
            availability: None,
            url: Url::parse("https://shop.example/listing")
                .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            images: IndexSet::new(),
            auction,
        })
        .unwrap_or_else(|error| panic!("valid listing should be created: {error}"));
        listing.take_pending_event_payload();
        listing
            .change_url(
                Url::parse("https://shop.example/updated-listing")
                    .unwrap_or_else(|error| panic!("invalid URL: {error}")),
            )
            .unwrap_or_else(|error| panic!("valid URL update: {error}"));

        let result = apply_command(
            &mut listing,
            UpdateProductListingCommand {
                auction_start: PatchField::Set(Some(time::macros::datetime!(2026-01-03 0:00 UTC))),
                ..Default::default()
            },
        );

        assert!(matches!(
            result,
            Err(UpdateProductListingError::InvalidProductListing)
        ));
        assert_eq!(listing.auction(), auction);
        let Some(ProductListingEventPayload::Changed(change)) =
            listing.take_pending_event_payload()
        else {
            panic!("expected changed payload");
        };
        assert!(change.url().is_some());
    }
}

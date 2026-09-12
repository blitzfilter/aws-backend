use crate::{
    metadata_acceptance::{EmbeddedAuctionMetadata, apply_embedded_auction_metadata},
    ports::{
        AuctionEventAppender, AuctionEventAppenderFactory, AuctionMetadataPolicyRepository,
        AuctionMetadataPolicyRepositoryFactory, AuctionRepository, AuctionRepositoryError,
        AuctionRepositoryFactory, stamp_auction_event,
    },
};
use application::error::{BoxError, box_error};
use auction_core::{Auction, AuctionId, AuctionKey, AuctionSchedule, NewAuction, SourceAuctionId};
use domain_primitives::change_outcome::ChangeOutcome;
use domain_primitives::event_id::EventId;
use listing_source_core::ListingSourceId;
use time::OffsetDateTime;

/// Result of resolving one reliable source auction reference inside a caller-owned transaction.
/// It deliberately exposes no adapter data or raw revision identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuctionWriteReceipt {
    pub auction_id: AuctionId,
    pub auction_event_id: Option<EventId>,
    pub outcome: ChangeOutcome,
    pub created: bool,
}

/// Caller-selected source context for one transactional listing resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolveAuctionForListingRequest {
    pub listing_source_id: ListingSourceId,
    pub source_auction_id: SourceAuctionId,
    pub current_membership: Option<AuctionId>,
    pub metadata: EmbeddedAuctionMetadata,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveAuctionForListingError {
    #[error("listing source does not exist")]
    ListingSourceNotFound,
    #[error("concurrent auction resolution")]
    ConcurrencyConflict,
    #[error("auction membership change requires an explicit correction")]
    MembershipChangeRequiresCorrection,
    #[error("auction persistence is temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("persisted auction state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("auction resolution failed internally")]
    Internal {
        #[source]
        source: BoxError,
    },
}

/// Narrow internal capability for the canonical listing writer.
///
/// The caller owns the transaction and must decide membership eligibility before invoking this
/// function. Embedded metadata is fill-only and is never an authority signal.
pub async fn resolve_auction_for_listing<Tx, R, E, P>(
    tx: &mut Tx,
    auctions: &R,
    events: &E,
    policies: &P,
    request: ResolveAuctionForListingRequest,
) -> Result<AuctionWriteReceipt, ResolveAuctionForListingError>
where
    R: AuctionRepositoryFactory<Tx>,
    E: AuctionEventAppenderFactory<Tx>,
    P: AuctionMetadataPolicyRepositoryFactory<Tx>,
{
    let key = AuctionKey::new(request.listing_source_id, request.source_auction_id);
    let existing = match request.current_membership {
        Some(auction_id) => {
            let stored = auctions
                .in_transaction(tx)
                .find_by_id(auction_id)
                .await
                .map_err(map_repository_error)?
                .ok_or_else(|| ResolveAuctionForListingError::InvalidPersistedState {
                    source: box_error(std::io::Error::other(
                        "listing auction membership references a missing auction",
                    )),
                })?;
            if stored.auction.key() != &key {
                return Err(ResolveAuctionForListingError::MembershipChangeRequiresCorrection);
            }
            Some(stored)
        }
        None => auctions
            .in_transaction(tx)
            .find_by_key(&key)
            .await
            .map_err(map_repository_error)?,
    };

    let Some(stored) = existing else {
        let mut auction = Auction::create(NewAuction {
            id: AuctionId::new(),
            key,
            name: request.metadata.name.clone(),
            description: request.metadata.description.clone(),
            catalogue_url: request.metadata.catalogue_url.clone(),
            format: request.metadata.format,
            schedule: AuctionSchedule::new(
                request.metadata.bidding_opens.clone(),
                request.metadata.live_starts.clone(),
                request.metadata.lots_begin_closing.clone(),
                request.metadata.scheduled_end.clone(),
            )
            .map_err(|error| ResolveAuctionForListingError::Internal {
                source: box_error(error),
            })?,
            reported_status: request.metadata.reported_status,
            reported_lot_count: request.metadata.reported_lot_count,
        })
        .map_err(|error| ResolveAuctionForListingError::Internal {
            source: box_error(error),
        })?;
        let event = stamp_auction_event(
            auction.id(),
            OffsetDateTime::now_utc(),
            auction.take_pending_event_payload().ok_or_else(|| {
                ResolveAuctionForListingError::Internal {
                    source: box_error(std::io::Error::other("new auction has no discovery event")),
                }
            })?,
        );
        let stored = auctions
            .in_transaction(tx)
            .insert(&auction)
            .await
            .map_err(map_repository_error)?;
        events
            .in_transaction(tx)
            .append(&event)
            .await
            .map_err(|error| ResolveAuctionForListingError::Internal {
                source: box_error(error),
            })?;
        return Ok(AuctionWriteReceipt {
            auction_id: stored.auction.id(),
            auction_event_id: Some(event.event_id),
            outcome: ChangeOutcome::Changed,
            created: true,
        });
    };

    let protected = policies
        .in_transaction(tx)
        .find_protected_fields(stored.auction.id())
        .await
        .map_err(|error| ResolveAuctionForListingError::Internal {
            source: box_error(error),
        })?;
    let mut auction = stored.auction;
    let acceptance = apply_embedded_auction_metadata(&mut auction, &protected, &request.metadata)
        .map_err(|error| ResolveAuctionForListingError::Internal {
        source: box_error(error),
    })?;
    if acceptance.change == ChangeOutcome::Unchanged {
        return Ok(AuctionWriteReceipt {
            auction_id: auction.id(),
            auction_event_id: None,
            outcome: ChangeOutcome::Unchanged,
            created: false,
        });
    }
    let event = stamp_auction_event(
        auction.id(),
        OffsetDateTime::now_utc(),
        auction.take_pending_event_payload().ok_or_else(|| {
            ResolveAuctionForListingError::Internal {
                source: box_error(std::io::Error::other("changed auction has no event")),
            }
        })?,
    );
    let stored = auctions
        .in_transaction(tx)
        .update(&auction, stored.version)
        .await
        .map_err(map_repository_error)?;
    events
        .in_transaction(tx)
        .append(&event)
        .await
        .map_err(|error| ResolveAuctionForListingError::Internal {
            source: box_error(error),
        })?;
    Ok(AuctionWriteReceipt {
        auction_id: stored.auction.id(),
        auction_event_id: Some(event.event_id),
        outcome: ChangeOutcome::Changed,
        created: false,
    })
}

fn map_repository_error(error: AuctionRepositoryError) -> ResolveAuctionForListingError {
    match error {
        AuctionRepositoryError::ListingSourceNotFound { .. } => {
            ResolveAuctionForListingError::ListingSourceNotFound
        }
        AuctionRepositoryError::ConcurrencyConflict
        | AuctionRepositoryError::SourceAuctionAlreadyExists { .. } => {
            ResolveAuctionForListingError::ConcurrencyConflict
        }
        AuctionRepositoryError::TemporarilyUnavailable { source } => {
            ResolveAuctionForListingError::TemporarilyUnavailable { source }
        }
        AuctionRepositoryError::InvalidPersistedState { source } => {
            ResolveAuctionForListingError::InvalidPersistedState { source }
        }
        AuctionRepositoryError::Internal { source } => {
            ResolveAuctionForListingError::Internal { source }
        }
    }
}

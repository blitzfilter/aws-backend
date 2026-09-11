use crate::SourceAuctionId;
use listing_source_core::ListingSourceId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuctionKey {
    listing_source_id: ListingSourceId,
    source_auction_id: SourceAuctionId,
}

impl AuctionKey {
    pub const fn new(
        listing_source_id: ListingSourceId,
        source_auction_id: SourceAuctionId,
    ) -> Self {
        Self {
            listing_source_id,
            source_auction_id,
        }
    }

    pub const fn listing_source_id(&self) -> ListingSourceId {
        self.listing_source_id
    }

    pub fn source_auction_id(&self) -> &SourceAuctionId {
        &self.source_auction_id
    }
}

impl PartialOrd for AuctionKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AuctionKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.listing_source_id.as_uuid(), &self.source_auction_id)
            .cmp(&(other.listing_source_id.as_uuid(), &other.source_auction_id))
    }
}

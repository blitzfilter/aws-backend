use crate::ports::{
    PublicAuctionDetails, PublicAuctionDetailsReadError, PublicAuctionDetailsReader,
};
use application::{error::BoxError, operation_context::OperationContext};
use auction_core::AuctionId;

pub type GetPublicAuctionResult = PublicAuctionDetails;

#[derive(Debug, thiserror::Error)]
pub enum GetPublicAuctionError {
    #[error("Auction not found")]
    NotFound,
    #[error("public Auction details are temporarily unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("public Auction details are invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GetPublicAuctionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        auction_id: AuctionId,
    ) -> Result<GetPublicAuctionResult, GetPublicAuctionError>;
}

pub struct GetPublicAuctionHandler<R> {
    details: R,
}

impl<R> GetPublicAuctionHandler<R> {
    pub fn new(details: R) -> Self {
        Self { details }
    }
}

#[async_trait::async_trait]
impl<R> GetPublicAuctionUseCase for GetPublicAuctionHandler<R>
where
    R: PublicAuctionDetailsReader,
{
    #[tracing::instrument(
        name = "get_public_auction",
        skip_all,
        fields(
            auction_id = %auction_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        auction_id: AuctionId,
    ) -> Result<GetPublicAuctionResult, GetPublicAuctionError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        self.details
            .find_by_id(auction_id)
            .await?
            .ok_or(GetPublicAuctionError::NotFound)
    }
}

impl From<PublicAuctionDetailsReadError> for GetPublicAuctionError {
    fn from(error: PublicAuctionDetailsReadError) -> Self {
        match error {
            PublicAuctionDetailsReadError::QueryFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PublicAuctionDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

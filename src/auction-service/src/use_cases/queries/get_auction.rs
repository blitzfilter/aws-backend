use crate::ports::{
    AuctionDetailsReadError, AuctionDetailsReader, AuctionMetadataField, AuctionStorageVersion,
};
use application::{
    error::{BoxError, static_error},
    operation_context::{OperationContext, Principal},
};
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionId, AuctionKey, AuctionName, AuctionReportedStatus,
    AuctionSchedule, ReportedCatalogueLotCount,
};
use localization::{Language, Localized};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use url::Url;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub struct AuctionAdminDetailsView {
    pub auction_id: AuctionId,
    pub key: AuctionKey,
    pub name: Option<Localized<Language, AuctionName>>,
    pub description: Option<Localized<Language, AuctionDescription>>,
    pub catalogue_url: Option<Url>,
    pub format: Option<AuctionFormat>,
    pub schedule: AuctionSchedule,
    pub reported_status: Option<AuctionReportedStatus>,
    pub reported_lot_count: Option<ReportedCatalogueLotCount>,
    pub version: AuctionStorageVersion,
    pub protected_fields: BTreeSet<AuctionMetadataField>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAuctionError {
    #[error("authenticated actor required to get auction")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("auction not found")]
    NotFound,
    #[error("temporary auction persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted auction state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal auction failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GetAuctionUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        auction_id: AuctionId,
    ) -> Result<AuctionAdminDetailsView, GetAuctionError>;
}

pub struct GetAuctionHandler<R, A> {
    details: R,
    check_user_admin: A,
}

impl<R, A> GetAuctionHandler<R, A> {
    pub fn new(details: R, check_user_admin: A) -> Self {
        Self {
            details,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<R, A> GetAuctionUseCase for GetAuctionHandler<R, A>
where
    R: AuctionDetailsReader,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(name = "get_auction", skip_all, fields(auction_id = %auction_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        auction_id: AuctionId,
    ) -> Result<AuctionAdminDetailsView, GetAuctionError> {
        ensure_admin(
            context,
            &self.check_user_admin,
            map_admin_error_for_get,
            GetAuctionError::AuthenticatedActorRequired,
        )
        .await?;
        let details = self
            .details
            .find_by_id(auction_id)
            .await?
            .ok_or(GetAuctionError::NotFound)?;
        Ok(AuctionAdminDetailsView::from_details(details))
    }
}

impl AuctionAdminDetailsView {
    pub fn from_details(details: crate::ports::AuctionDetails) -> Self {
        let stored = details.stored;
        let auction = stored.auction;
        Self {
            auction_id: auction.id(),
            key: auction.key().clone(),
            name: auction.name().cloned(),
            description: auction.description().cloned(),
            catalogue_url: auction.catalogue_url().cloned(),
            format: auction.format(),
            schedule: auction.schedule().clone(),
            reported_status: auction.reported_status(),
            reported_lot_count: auction.reported_lot_count(),
            version: stored.version,
            protected_fields: details.protected_fields,
            created: stored.created,
            updated: stored.updated,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuctionAuditActorLabelError {
    #[error("auction audit actor label is empty")]
    Empty,
    #[error("auction audit actor label contains NUL")]
    Nul,
    #[error("auction audit actor label exceeds 512 UTF-8 bytes")]
    TooLong,
}

pub(crate) fn auction_audit_actor_label(
    context: &OperationContext,
) -> Result<String, AuctionAuditActorLabelError> {
    let label = context.principal.label();
    if label.is_empty() {
        return Err(AuctionAuditActorLabelError::Empty);
    }
    if label.contains('\0') {
        return Err(AuctionAuditActorLabelError::Nul);
    }
    if label.len() > 512 {
        return Err(AuctionAuditActorLabelError::TooLong);
    }
    Ok(label)
}

pub(crate) async fn ensure_admin<A, E>(
    context: &OperationContext,
    check: &A,
    map: impl Fn(CheckUserAdminError) -> E,
    unauthenticated: E,
) -> Result<(), E>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous => Err(unauthenticated),
        Principal::User(_) | Principal::DelegatedUser { .. } => check
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(map),
    }
}

pub(crate) fn map_admin_error_for_get(error: CheckUserAdminError) -> GetAuctionError {
    match error {
        CheckUserAdminError::AuthenticatedActorRequired => {
            GetAuctionError::AuthenticatedActorRequired
        }
        CheckUserAdminError::Forbidden => GetAuctionError::Forbidden,
        CheckUserAdminError::TemporarilyUnavailable { source } => {
            GetAuctionError::TemporarilyUnavailable { source }
        }
        CheckUserAdminError::InvalidReadModel { source }
        | CheckUserAdminError::Internal { source } => GetAuctionError::Internal { source },
        CheckUserAdminError::BeginTransactionFailed
        | CheckUserAdminError::CommitTransactionFailed => GetAuctionError::TemporarilyUnavailable {
            source: static_error("check user admin transaction failed"),
        },
    }
}

impl From<AuctionDetailsReadError> for GetAuctionError {
    fn from(error: AuctionDetailsReadError) -> Self {
        match error {
            AuctionDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionDetailsReadError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AuctionDetailsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

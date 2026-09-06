use crate::{
    admin_authorization::{AdminAuthorizationError, authorize_admin},
    ports::*,
};
use application::{
    error::{BoxError, static_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::{ListingSource, ListingSourceId, NewListingSource};
use listing_source_service::ports::{
    ListingIngestionConfiguration, ListingSourceIngestionConfigurations,
};
use notification_core::notification::{
    Notification, NotificationContent, PartnershipApplicationDecision,
    PartnershipApplicationNotificationSnapshot,
};
use notification_core::notification_id::NotificationId;
use notification_service::ports::notification_creator::{
    ExternalDeliveryRequest, NewNotification, NotificationCreationError, NotificationCreator,
    NotificationCreatorFactory,
};
use partnership_core::{
    partnership_application::{
        PartnershipApplication, PartnershipApplicationApprovalResult, PartnershipProposal,
    },
    partnership_application_id::PartnershipApplicationId,
    partnership_application_state::PartnershipApplicationState,
    partnership_id::PartnershipId,
};
use party_core::{
    party::{NewParty, Party},
    party_id::PartyId,
};
use user_service::ports::UserAdminReaderFactory;

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovePartnershipApplicationCommand {
    pub application_id: PartnershipApplicationId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovePartnershipApplicationResult {
    pub application: PartnershipApplication,
    pub partnership_id: Option<PartnershipId>,
    pub listing_source_id: Option<ListingSourceId>,
}
#[derive(Debug, thiserror::Error)]
pub enum ApprovePartnershipApplicationError {
    #[error("operation not permitted")]
    Forbidden,
    #[error("partnership application not found")]
    NotFound,
    #[error("partnership application is not approvable")]
    ApplicationNotApprovable,
    #[error("existing listing source not found")]
    ListingSourceNotFound,
    #[error("concurrent partnership application update")]
    ConcurrencyConflict,
    #[error("partnership application notification creation failed")]
    NotificationCreateFailed {
        #[source]
        source: BoxError,
    },
    #[error("party or listing source slug conflict")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary partnership approval failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal partnership approval failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin transaction")]
    BeginTransactionFailed,
    #[error("failed to commit transaction")]
    CommitTransactionFailed,
}
#[async_trait::async_trait]
pub trait ApprovePartnershipApplicationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ApprovePartnershipApplicationCommand,
    ) -> Result<ApprovePartnershipApplicationResult, ApprovePartnershipApplicationError>;
}
pub struct ApprovePartnershipApplicationHandler<U, A, P, S, R, M, G, N, C> {
    unit_of_work: U,
    applications: A,
    parties: P,
    sources: S,
    partnerships: R,
    memberships: M,
    grants: G,
    admins: N,
    notifications: C,
}
impl<U, A, P, S, R, M, G, N, C> ApprovePartnershipApplicationHandler<U, A, P, S, R, M, G, N, C> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        unit_of_work: U,
        applications: A,
        parties: P,
        sources: S,
        partnerships: R,
        memberships: M,
        grants: G,
        admins: N,
        notifications: C,
    ) -> Self {
        Self {
            unit_of_work,
            applications,
            parties,
            sources,
            partnerships,
            memberships,
            grants,
            admins,
            notifications,
        }
    }
}
#[async_trait::async_trait]
impl<U, A, P, S, R, M, G, N, C> ApprovePartnershipApplicationUseCase
    for ApprovePartnershipApplicationHandler<U, A, P, S, R, M, G, N, C>
where
    U: UnitOfWork,
    A: PartnershipApplicationRepositoryFactory<U::Tx>,
    P: PartyRepositoryFactory<U::Tx>,
    S: ListingSourceRepositoryFactory<U::Tx>,
    R: PartnershipRepositoryFactory<U::Tx>,
    M: PartnershipMembershipRepositoryFactory<U::Tx>,
    G: ListingSourceGrantRepositoryFactory<U::Tx>,
    N: UserAdminReaderFactory<U::Tx>,
    C: NotificationCreatorFactory<U::Tx>,
{
    #[tracing::instrument(name="approve_partnership_application",skip_all,fields(partnership_application_id=%command.application_id,principal_type=context.principal.kind(),request_id=%context.request_id,correlation_id=%context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ApprovePartnershipApplicationCommand,
    ) -> Result<ApprovePartnershipApplicationResult, ApprovePartnershipApplicationError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| ApprovePartnershipApplicationError::BeginTransactionFailed)?;
        authorize_admin(context, &mut tx, &self.admins).await?;
        let mut versioned = self
            .applications
            .in_transaction(&mut tx)
            .find_by_id_for_update(command.application_id)
            .await?
            .ok_or(ApprovePartnershipApplicationError::NotFound)?;
        if versioned.value.state() == PartnershipApplicationState::Approved {
            let approval_result = versioned.value.approval_result().ok_or(
                ApprovePartnershipApplicationError::InvalidPersistedState {
                    source: static_error("approved application missing approval result"),
                },
            )?;
            tx.commit()
                .await
                .map_err(|_| ApprovePartnershipApplicationError::CommitTransactionFailed)?;
            return Ok(ApprovePartnershipApplicationResult {
                application: versioned.value,
                partnership_id: Some(approval_result.partnership_id()),
                listing_source_id: Some(approval_result.listing_source_id()),
            });
        }
        if versioned.value.state() != PartnershipApplicationState::InReview {
            return Err(ApprovePartnershipApplicationError::ApplicationNotApprovable);
        }
        let (party_id, party_name, source_id, source_name, image) =
            match versioned.value.proposal().clone() {
                PartnershipProposal::ExistingListingSource { listing_source_id } => {
                    let source = self
                        .sources
                        .in_transaction(&mut tx)
                        .find_by_id(listing_source_id)
                        .await?
                        .ok_or(ApprovePartnershipApplicationError::ListingSourceNotFound)?
                        .source;
                    let party = self
                        .parties
                        .in_transaction(&mut tx)
                        .find_by_id(source.operator_party_id())
                        .await?
                        .ok_or(ApprovePartnershipApplicationError::ListingSourceNotFound)?
                        .party;
                    (
                        party.id(),
                        party.name().clone(),
                        source.id(),
                        source.name().clone(),
                        source.presentation().image.clone(),
                    )
                }
                PartnershipProposal::ProposedListingSource {
                    party,
                    listing_source,
                } => {
                    let party = Party::create(NewParty {
                        id: PartyId::new(),
                        name: party.name,
                        contact: party.contact,
                    });
                    let party = self
                        .parties
                        .in_transaction(&mut tx)
                        .insert(&party)
                        .await?
                        .party;
                    let config = ListingSourceIngestionConfigurations(
                        listing_source
                            .requested_ingestion_methods
                            .iter()
                            .filter_map(|method| match method {
                                listing_source_core::ListingIngestionMethod::WebCrawl => {
                                    Some(ListingIngestionConfiguration::WebCrawl)
                                }
                                listing_source_core::ListingIngestionMethod::PartnerApi => {
                                    Some(ListingIngestionConfiguration::PartnerApi)
                                }
                                listing_source_core::ListingIngestionMethod::Shopify
                                | listing_source_core::ListingIngestionMethod::Woocommerce => None,
                            })
                            .collect(),
                    );
                    let source = ListingSource::create(NewListingSource {
                        id: ListingSourceId::new(),
                        name: listing_source.name,
                        operator_party_id: party.id(),
                        ingestion_methods: config.methods().map_err(|source| {
                            ApprovePartnershipApplicationError::InvalidPersistedState {
                                source: application::error::box_error(source),
                            }
                        })?,
                        presentation: listing_source.presentation,
                        referral_configuration: None,
                    });
                    let source = self
                        .sources
                        .in_transaction(&mut tx)
                        .insert(&source, &config, None)
                        .await?
                        .source;
                    (
                        party.id(),
                        party.name().clone(),
                        source.id(),
                        source.name().clone(),
                        source.presentation().image.clone(),
                    )
                }
            };
        let partnership = self
            .partnerships
            .in_transaction(&mut tx)
            .find_or_create_for_party(party_id, PartnershipId::new())
            .await?
            .value;
        self.memberships
            .in_transaction(&mut tx)
            .add_member(versioned.value.applicant_user_id(), partnership.id())
            .await?;
        self.grants
            .in_transaction(&mut tx)
            .grant_source_access(partnership.id(), source_id)
            .await?;
        versioned
            .value
            .approve(PartnershipApplicationApprovalResult::new(
                partnership.id(),
                source_id,
            ))
            .map_err(|_| ApprovePartnershipApplicationError::ApplicationNotApprovable)?;
        let application = self
            .applications
            .in_transaction(&mut tx)
            .update(&versioned.value, versioned.version)
            .await?
            .value;
        self.notifications
            .in_transaction(&mut tx)
            .create_many(&[approval_notification(
                application.id(),
                application.applicant_user_id(),
                party_name,
                source_name,
                image,
            )])
            .await
            .map_err(|source: NotificationCreationError| {
                ApprovePartnershipApplicationError::NotificationCreateFailed {
                    source: application::error::box_error(source),
                }
            })?;
        tx.commit()
            .await
            .map_err(|_| ApprovePartnershipApplicationError::CommitTransactionFailed)?;
        tracing::info!(event="partnership_application.approved",partnership_application_id=%application.id(),partnership_id=%partnership.id(),listing_source_id=%source_id,actor_type=context.principal.kind(),outcome="success");
        Ok(ApprovePartnershipApplicationResult {
            application,
            partnership_id: Some(partnership.id()),
            listing_source_id: Some(source_id),
        })
    }
}
fn approval_notification(
    application_id: PartnershipApplicationId,
    applicant_user_id: user_core::user_id::UserId,
    party_name: party_core::party_name::PartyName,
    listing_source_name: listing_source_core::ListingSourceName,
    image: Option<url::Url>,
) -> NewNotification {
    NewNotification {
        notification: Notification::new(
            NotificationId::new(),
            applicant_user_id,
            NotificationContent::PartnershipApplication {
                partnership_application_id: application_id,
                snapshot: PartnershipApplicationNotificationSnapshot {
                    party_name,
                    listing_source_name,
                    image,
                },
                decision: PartnershipApplicationDecision::Approved,
            },
        ),
        external_delivery: ExternalDeliveryRequest::Requested,
    }
}

impl From<AdminAuthorizationError> for ApprovePartnershipApplicationError {
    fn from(value: AdminAuthorizationError) -> Self {
        match value {
            AdminAuthorizationError::Forbidden => Self::Forbidden,
            AdminAuthorizationError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AdminAuthorizationError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            AdminAuthorizationError::Internal { source } => Self::Internal { source },
        }
    }
}
impl From<PartnershipApplicationRepositoryError> for ApprovePartnershipApplicationError {
    fn from(value: PartnershipApplicationRepositoryError) -> Self {
        match value {
            PartnershipApplicationRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            PartnershipApplicationRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipApplicationRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipApplicationRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
impl From<PartnershipRepositoryError> for ApprovePartnershipApplicationError {
    fn from(value: PartnershipRepositoryError) -> Self {
        match value {
            PartnershipRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            PartnershipRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}
impl From<PartnershipGrantError> for ApprovePartnershipApplicationError {
    fn from(value: PartnershipGrantError) -> Self {
        match value {
            PartnershipGrantError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PartnershipGrantError::Internal { source } => Self::Internal { source },
        }
    }
}
impl From<party_service::ports::PartyRepositoryError> for ApprovePartnershipApplicationError {
    fn from(value: party_service::ports::PartyRepositoryError) -> Self {
        match value {
            party_service::ports::PartyRepositoryError::SlugConflict { source } => {
                Self::SlugConflict { source }
            }
            party_service::ports::PartyRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            party_service::ports::PartyRepositoryError::InvalidPersistedState { source }
            | party_service::ports::PartyRepositoryError::Internal { source } => {
                Self::Internal { source }
            }
            party_service::ports::PartyRepositoryError::ConcurrencyConflict => Self::Internal {
                source: static_error("unexpected party concurrency"),
            },
        }
    }
}
impl From<listing_source_service::ports::ListingSourceRepositoryError>
    for ApprovePartnershipApplicationError
{
    fn from(value: listing_source_service::ports::ListingSourceRepositoryError) -> Self {
        match value {
            listing_source_service::ports::ListingSourceRepositoryError::SlugConflict { source }
            | listing_source_service::ports::ListingSourceRepositoryError::ShopifyDomainConflict {
                source,
            } => Self::SlugConflict { source },
            listing_source_service::ports::ListingSourceRepositoryError::TemporarilyUnavailable {
                source,
            } => Self::TemporarilyUnavailable { source },
            listing_source_service::ports::ListingSourceRepositoryError::InvalidPersistedState {
                source,
            }
            | listing_source_service::ports::ListingSourceRepositoryError::Internal { source } => {
                Self::Internal { source }
            }
            listing_source_service::ports::ListingSourceRepositoryError::ConcurrencyConflict => {
                Self::Internal {
                    source: static_error("unexpected listing source concurrency"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::box_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use domain_primitives::versioned::Versioned;
    use listing_source_core::{
        ListingIngestionMethod, ListingSourceName, ListingSourcePresentation,
    };
    use listing_source_service::ports::{
        ListingSourceRepository, ListingSourceRepositoryError, ListingSourceStorageVersion,
        StoredListingSource,
    };
    use notification_service::ports::notification_creator::NotificationCreationOutcome;
    use partnership_core::partnership::{NewPartnership, Partnership};
    use party_core::{
        party::{Party, PartyContact},
        party_name::PartyName,
    };
    use party_service::ports::{PartyRepositoryError, PartyStorageVersion, StoredParty};
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex, MutexGuard},
    };
    use time::OffsetDateTime;
    use user_service::ports::{UserAdminActorView, UserAdminReadError, UserAdminReader};

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<FakeState>>,
    }

    struct FakeTransaction {
        state: Arc<Mutex<FakeState>>,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.state);
            state.commit_attempts += 1;
            if state.commit_fails {
                Err(TransactionError::CommitFailed)
            } else {
                state.committed += 1;
                Ok(())
            }
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.state).begun += 1;
            Ok(FakeTransaction {
                state: Arc::clone(&self.state),
            })
        }
    }

    #[derive(Clone)]
    struct FakeFactories {
        state: Arc<Mutex<FakeState>>,
    }

    struct FakeApplicationRepository {
        state: Arc<Mutex<FakeState>>,
    }
    struct FakePartyRepository {
        state: Arc<Mutex<FakeState>>,
    }
    struct FakeSourceRepository {
        state: Arc<Mutex<FakeState>>,
    }
    struct FakePartnershipRepository {
        state: Arc<Mutex<FakeState>>,
    }
    struct FakeMembershipRepository {
        state: Arc<Mutex<FakeState>>,
    }
    struct FakeGrantRepository {
        state: Arc<Mutex<FakeState>>,
    }
    struct FakeAdminReader;
    struct FakeNotificationCreator {
        state: Arc<Mutex<FakeState>>,
    }

    macro_rules! factory {
        ($factory:ident, $repository_trait:ident, $repository:ident) => {
            impl $factory<FakeTransaction> for FakeFactories {
                fn in_transaction<'tx>(
                    &'tx self,
                    _tx: &'tx mut FakeTransaction,
                ) -> impl $repository_trait + 'tx {
                    $repository {
                        state: Arc::clone(&self.state),
                    }
                }
            }
        };
    }

    factory!(
        PartnershipApplicationRepositoryFactory,
        PartnershipApplicationRepository,
        FakeApplicationRepository
    );
    factory!(PartyRepositoryFactory, PartyRepository, FakePartyRepository);
    factory!(
        ListingSourceRepositoryFactory,
        ListingSourceRepository,
        FakeSourceRepository
    );
    factory!(
        PartnershipRepositoryFactory,
        PartnershipRepository,
        FakePartnershipRepository
    );
    factory!(
        PartnershipMembershipRepositoryFactory,
        PartnershipMembershipRepository,
        FakeMembershipRepository
    );
    factory!(
        ListingSourceGrantRepositoryFactory,
        ListingSourceGrantRepository,
        FakeGrantRepository
    );

    impl UserAdminReaderFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl UserAdminReader + 'tx {
            FakeAdminReader
        }
    }

    impl NotificationCreatorFactory<FakeTransaction> for FakeFactories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl NotificationCreator + 'tx {
            FakeNotificationCreator {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl UserAdminReader for FakeAdminReader {
        async fn find_admin_actor(
            &mut self,
            _user_id: user_core::user_id::UserId,
        ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
            Ok(None)
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for FakeNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            let mut state = lock(&self.state);
            state.notification_calls += 1;
            if state.notification_fails {
                return Err(NotificationCreationError::CreateFailed {
                    source: static_error("notification creation failed"),
                });
            }
            state
                .notifications
                .extend(notifications.iter().map(|item| item.notification.clone()));
            Ok(notifications
                .iter()
                .map(|_| NotificationCreationOutcome::Inserted {
                    notification_id: NotificationId::new(),
                })
                .collect())
        }
    }

    #[async_trait::async_trait]
    impl PartnershipApplicationRepository for FakeApplicationRepository {
        async fn find_by_id(
            &mut self,
            _id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            let state = lock(&self.state);
            let version =
                PartnershipApplicationStorageVersion::try_from(1_i64).map_err(|error| {
                    PartnershipApplicationRepositoryError::Internal {
                        source: box_error(error),
                    }
                })?;
            Ok(state
                .application
                .clone()
                .map(|application| Versioned::new(application, version)))
        }

        async fn find_by_id_for_update(
            &mut self,
            id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            self.find_by_id(id).await
        }

        async fn find_by_user_and_id(
            &mut self,
            user_id: user_core::user_id::UserId,
            id: PartnershipApplicationId,
        ) -> Result<Option<VersionedPartnershipApplication>, PartnershipApplicationRepositoryError>
        {
            let found = self.find_by_id(id).await?;
            Ok(found.filter(|application| application.value.applicant_user_id() == user_id))
        }

        async fn insert(
            &mut self,
            application: &PartnershipApplication,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            lock(&self.state).application = Some(application.clone());
            let version =
                PartnershipApplicationStorageVersion::try_from(1_i64).map_err(|error| {
                    PartnershipApplicationRepositoryError::Internal {
                        source: box_error(error),
                    }
                })?;
            Ok(Versioned::new(application.clone(), version))
        }

        async fn update(
            &mut self,
            application: &PartnershipApplication,
            _expected: PartnershipApplicationStorageVersion,
        ) -> Result<VersionedPartnershipApplication, PartnershipApplicationRepositoryError>
        {
            let mut state = lock(&self.state);
            state.application_updates += 1;
            state.application = Some(application.clone());
            let version =
                PartnershipApplicationStorageVersion::try_from(2_i64).map_err(|error| {
                    PartnershipApplicationRepositoryError::Internal {
                        source: box_error(error),
                    }
                })?;
            Ok(Versioned::new(application.clone(), version))
        }
    }

    #[async_trait::async_trait]
    impl PartyRepository for FakePartyRepository {
        async fn find_by_id(
            &mut self,
            id: PartyId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Ok(lock(&self.state)
                .existing_party
                .clone()
                .filter(|party| party.party.id() == id))
        }

        async fn find_by_slug(
            &mut self,
            _slug_id: &party_core::party_slug_id::PartySlugId,
        ) -> Result<Option<StoredParty>, PartyRepositoryError> {
            Ok(None)
        }

        async fn insert(&mut self, party: &Party) -> Result<StoredParty, PartyRepositoryError> {
            let mut state = lock(&self.state);
            if state.party_insert_fails {
                return Err(PartyRepositoryError::SlugConflict {
                    source: static_error("party slug already exists"),
                });
            }
            state.party_inserts += 1;
            let version = PartyStorageVersion::try_from(1_i64).map_err(|error| {
                PartyRepositoryError::Internal {
                    source: box_error(error),
                }
            })?;
            Ok(StoredParty {
                party: party.clone(),
                version,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            })
        }

        async fn update(
            &mut self,
            _party: &Party,
            _expected_version: PartyStorageVersion,
        ) -> Result<StoredParty, PartyRepositoryError> {
            Err(PartyRepositoryError::Internal {
                source: static_error("unexpected party update"),
            })
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceRepository for FakeSourceRepository {
        async fn find_by_id(
            &mut self,
            id: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(lock(&self.state)
                .existing_source
                .clone()
                .filter(|source| source.source.id() == id))
        }

        async fn find_by_slug(
            &mut self,
            _slug: &listing_source_core::ListingSourceSlugId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(None)
        }

        async fn insert(
            &mut self,
            source: &ListingSource,
            configuration: &ListingSourceIngestionConfigurations,
            _woocommerce_webhook_secret: Option<&str>,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            let mut state = lock(&self.state);
            state.source_inserts += 1;
            let version = ListingSourceStorageVersion::try_from(1_i64).map_err(|error| {
                ListingSourceRepositoryError::Internal {
                    source: box_error(error),
                }
            })?;
            let stored = StoredListingSource {
                source: source.clone(),
                configuration: configuration.clone(),
                version,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            };
            state.existing_source = Some(stored.clone());
            Ok(stored)
        }

        async fn update(
            &mut self,
            _source: &ListingSource,
            _configuration: &ListingSourceIngestionConfigurations,
            _woocommerce_webhook_secret: application::patch_field::PatchField<&str>,
            _expected: ListingSourceStorageVersion,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(ListingSourceRepositoryError::Internal {
                source: static_error("unexpected listing source update"),
            })
        }
    }

    #[async_trait::async_trait]
    impl PartnershipRepository for FakePartnershipRepository {
        async fn find_by_id(
            &mut self,
            partnership_id: PartnershipId,
        ) -> Result<Option<VersionedPartnership>, PartnershipRepositoryError> {
            let state = lock(&self.state);
            let version = PartnershipStorageVersion::try_from(1_i64).map_err(|error| {
                PartnershipRepositoryError::Internal {
                    source: box_error(error),
                }
            })?;
            Ok(state
                .partnership
                .clone()
                .filter(|partnership| partnership.id() == partnership_id)
                .map(|partnership| Versioned::new(partnership, version)))
        }

        async fn find_or_create_for_party(
            &mut self,
            party_id: PartyId,
            new_partnership_id: PartnershipId,
        ) -> Result<VersionedPartnership, PartnershipRepositoryError> {
            let mut state = lock(&self.state);
            let partnership = state
                .partnership
                .clone()
                .filter(|partnership| partnership.party_id() == party_id)
                .unwrap_or_else(|| {
                    state.partnership_inserts += 1;
                    let partnership = Partnership::create(NewPartnership {
                        id: new_partnership_id,
                        party_id,
                    });
                    state.partnership = Some(partnership.clone());
                    partnership
                });
            let version = PartnershipStorageVersion::try_from(1_i64).map_err(|error| {
                PartnershipRepositoryError::Internal {
                    source: box_error(error),
                }
            })?;
            Ok(Versioned::new(partnership, version))
        }
    }

    #[async_trait::async_trait]
    impl PartnershipMembershipRepository for FakeMembershipRepository {
        async fn add_member(
            &mut self,
            user_id: user_core::user_id::UserId,
            partnership_id: PartnershipId,
        ) -> Result<PartnershipMembershipAddOutcome, PartnershipGrantError> {
            lock(&self.state)
                .memberships
                .insert((user_id, partnership_id));
            Ok(PartnershipMembershipAddOutcome::Added)
        }

        async fn remove_member(
            &mut self,
            user_id: user_core::user_id::UserId,
            partnership_id: PartnershipId,
        ) -> Result<PartnershipMembershipRemoveOutcome, PartnershipGrantError> {
            let removed = lock(&self.state)
                .memberships
                .remove(&(user_id, partnership_id));
            Ok(if removed {
                PartnershipMembershipRemoveOutcome::Removed
            } else {
                PartnershipMembershipRemoveOutcome::AlreadyAbsent
            })
        }
    }

    #[async_trait::async_trait]
    impl ListingSourceGrantRepository for FakeGrantRepository {
        async fn grant_source_access(
            &mut self,
            partnership_id: PartnershipId,
            listing_source_id: ListingSourceId,
        ) -> Result<ListingSourceGrantOutcome, PartnershipGrantError> {
            let added = lock(&self.state)
                .grants
                .insert((partnership_id, listing_source_id));
            Ok(if added {
                ListingSourceGrantOutcome::Granted
            } else {
                ListingSourceGrantOutcome::AlreadyGranted
            })
        }

        async fn remove_source_access(
            &mut self,
            partnership_id: PartnershipId,
            listing_source_id: ListingSourceId,
        ) -> Result<ListingSourceGrantRemoveOutcome, PartnershipGrantError> {
            let removed = lock(&self.state)
                .grants
                .remove(&(partnership_id, listing_source_id));
            Ok(if removed {
                ListingSourceGrantRemoveOutcome::Removed
            } else {
                ListingSourceGrantRemoveOutcome::AlreadyAbsent
            })
        }
    }

    struct FakeState {
        application: Option<PartnershipApplication>,
        existing_source: Option<StoredListingSource>,
        existing_party: Option<StoredParty>,
        partnership: Option<Partnership>,
        memberships: HashSet<(user_core::user_id::UserId, PartnershipId)>,
        grants: HashSet<(PartnershipId, ListingSourceId)>,
        party_insert_fails: bool,
        commit_fails: bool,
        begun: usize,
        commit_attempts: usize,
        committed: usize,
        party_inserts: usize,
        source_inserts: usize,
        partnership_inserts: usize,
        application_updates: usize,
        notification_calls: usize,
        notification_fails: bool,
        notifications: Vec<Notification>,
    }

    impl FakeState {
        fn with_application(application: PartnershipApplication) -> Self {
            Self {
                application: Some(application),
                existing_source: None,
                existing_party: None,
                partnership: None,
                memberships: HashSet::new(),
                grants: HashSet::new(),
                party_insert_fails: false,
                commit_fails: false,
                begun: 0,
                commit_attempts: 0,
                committed: 0,
                party_inserts: 0,
                source_inserts: 0,
                partnership_inserts: 0,
                application_updates: 0,
                notification_calls: 0,
                notification_fails: false,
                notifications: Vec::new(),
            }
        }
    }

    fn lock(state: &Arc<Mutex<FakeState>>) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn proposed_application() -> PartnershipApplication {
        PartnershipApplication::rehydrate(
            partnership_core::partnership_application::RehydratedPartnershipApplicationState {
                id: PartnershipApplicationId::new(),
                applicant_user_id: user_core::user_id::UserId::new(),
                state: PartnershipApplicationState::InReview,
                approval_result: None,
                proposal: PartnershipProposal::ProposedListingSource {
                    party: partnership_core::partnership_application::ProposedParty {
                        name: PartyName::try_from("Northwind Antiques")
                            .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
                        contact: PartyContact {
                            phone: None,
                            email: None,
                        },
                    },
                    listing_source:
                        partnership_core::partnership_application::ProposedListingSource {
                            name: ListingSourceName::try_from("Northwind Source").unwrap_or_else(
                                |error| panic!("invalid test listing source name: {error}"),
                            ),
                            presentation: ListingSourcePresentation {
                                url: None,
                                image: None,
                            },
                            requested_ingestion_methods: HashSet::from([
                                ListingIngestionMethod::PartnerApi,
                            ]),
                        },
                },
            },
        )
        .unwrap_or_else(|error| panic!("valid test application: {error}"))
    }

    fn handler(
        state: Arc<Mutex<FakeState>>,
    ) -> ApprovePartnershipApplicationHandler<
        FakeUnitOfWork,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
        FakeFactories,
    > {
        let factories = FakeFactories {
            state: Arc::clone(&state),
        };
        ApprovePartnershipApplicationHandler::new(
            FakeUnitOfWork { state },
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories.clone(),
            factories,
        )
    }

    #[tokio::test]
    async fn should_approve_proposed_source_atomically_and_replay_without_duplicate_grants() {
        let application = proposed_application();
        let application_id = application.id();
        let state = Arc::new(Mutex::new(FakeState::with_application(application)));
        let handler = handler(Arc::clone(&state));

        let first = handler
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;
        assert!(first.is_ok());
        let second = handler
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;
        assert!(second.is_ok());

        let state = lock(&state);
        assert_eq!(2, state.begun);
        assert_eq!(2, state.committed);
        assert_eq!(1, state.party_inserts);
        assert_eq!(1, state.source_inserts);
        assert_eq!(1, state.partnership_inserts);
        assert_eq!(1, state.application_updates);
        assert_eq!(1, state.notification_calls);
        assert!(matches!(
            state.notifications.as_slice(),
            [Notification { .. }]
        ));
        assert!(matches!(
            state.notifications.first().map(Notification::content),
            Some(NotificationContent::PartnershipApplication {
                decision: PartnershipApplicationDecision::Approved,
                ..
            })
        ));
        assert_eq!(1, state.memberships.len());
        assert_eq!(1, state.grants.len());
        assert_eq!(
            PartnershipApplicationState::Approved,
            state
                .application
                .as_ref()
                .map(PartnershipApplication::state)
                .unwrap_or(PartnershipApplicationState::Withdrawn)
        );
    }

    #[tokio::test]
    async fn should_approve_existing_source_without_creating_party_or_source() {
        let applicant_user_id = user_core::user_id::UserId::new();
        let party_id = PartyId::new();
        let party = Party::create(NewParty {
            id: party_id,
            name: PartyName::try_from("Existing Operator")
                .unwrap_or_else(|error| panic!("invalid test party name: {error}")),
            contact: PartyContact {
                phone: None,
                email: None,
            },
        });
        let source = ListingSource::create(NewListingSource {
            id: ListingSourceId::new(),
            name: ListingSourceName::try_from("Existing Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            operator_party_id: party_id,
            ingestion_methods: HashSet::from([ListingIngestionMethod::PartnerApi]),
            presentation: ListingSourcePresentation {
                url: None,
                image: None,
            },
            referral_configuration: None,
        });
        let source_version = match ListingSourceStorageVersion::try_from(1_i64) {
            Ok(version) => version,
            Err(error) => panic!("test listing source version: {error}"),
        };
        let party_version = match PartyStorageVersion::try_from(1_i64) {
            Ok(version) => version,
            Err(error) => panic!("test party version: {error}"),
        };
        let application = PartnershipApplication::rehydrate(
            partnership_core::partnership_application::RehydratedPartnershipApplicationState {
                id: PartnershipApplicationId::new(),
                applicant_user_id,
                state: PartnershipApplicationState::InReview,
                proposal: PartnershipProposal::ExistingListingSource {
                    listing_source_id: source.id(),
                },
                approval_result: None,
            },
        )
        .unwrap_or_else(|error| panic!("valid test application: {error}"));
        let application_id = application.id();
        let mut fake_state = FakeState::with_application(application);
        fake_state.existing_party = Some(StoredParty {
            party,
            version: party_version,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        });
        fake_state.existing_source = Some(StoredListingSource {
            source: source.clone(),
            configuration: ListingSourceIngestionConfigurations(vec![
                ListingIngestionConfiguration::PartnerApi,
            ]),
            version: source_version,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        });
        let state = Arc::new(Mutex::new(fake_state));

        let result = handler(Arc::clone(&state))
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(result.is_ok());
        let state = lock(&state);
        assert_eq!(0, state.party_inserts);
        assert_eq!(0, state.source_inserts);
        assert_eq!(1, state.memberships.len());
        assert_eq!(1, state.grants.len());
        assert_eq!(1, state.committed);
    }

    #[tokio::test]
    async fn should_leave_transaction_uncommitted_when_party_slug_conflicts() {
        let application = proposed_application();
        let application_id = application.id();
        let mut fake_state = FakeState::with_application(application);
        fake_state.party_insert_fails = true;
        let state = Arc::new(Mutex::new(fake_state));

        let result = handler(Arc::clone(&state))
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(ApprovePartnershipApplicationError::SlugConflict { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.begun);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(0, state.memberships.len());
        assert_eq!(0, state.grants.len());
        assert_eq!(0, state.application_updates);
    }

    #[tokio::test]
    async fn should_leave_transaction_uncommitted_when_notification_creation_fails() {
        let application = proposed_application();
        let application_id = application.id();
        let mut fake_state = FakeState::with_application(application);
        fake_state.notification_fails = true;
        let state = Arc::new(Mutex::new(fake_state));

        let result = handler(Arc::clone(&state))
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(ApprovePartnershipApplicationError::NotificationCreateFailed { .. })
        ));
        let state = lock(&state);
        assert_eq!(1, state.notification_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_report_commit_failure_after_successful_orchestration() {
        let application = proposed_application();
        let application_id = application.id();
        let mut fake_state = FakeState::with_application(application);
        fake_state.commit_fails = true;
        let state = Arc::new(Mutex::new(fake_state));

        let result = handler(Arc::clone(&state))
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(ApprovePartnershipApplicationError::CommitTransactionFailed)
        ));
        let state = lock(&state);
        assert_eq!(1, state.commit_attempts);
        assert_eq!(0, state.committed);
    }

    #[tokio::test]
    async fn should_reject_invalid_state_without_writes_or_commit() {
        let proposed = proposed_application();
        let application = PartnershipApplication::rehydrate(
            partnership_core::partnership_application::RehydratedPartnershipApplicationState {
                id: proposed.id(),
                applicant_user_id: proposed.applicant_user_id(),
                state: PartnershipApplicationState::Submitted,
                proposal: proposed.proposal().clone(),
                approval_result: None,
            },
        )
        .unwrap_or_else(|error| panic!("valid test application: {error}"));
        let application_id = application.id();
        let state = Arc::new(Mutex::new(FakeState::with_application(application)));

        let result = handler(Arc::clone(&state))
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(ApprovePartnershipApplicationError::ApplicationNotApprovable)
        ));
        let state = lock(&state);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(0, state.application_updates);
        assert_eq!(0, state.memberships.len());
        assert_eq!(0, state.grants.len());
    }
}

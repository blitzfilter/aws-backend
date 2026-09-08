use crate::ports::*;
use application::{
    error::{BoxError, static_error},
    operation_context::{OperationContext, Principal},
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::change_outcome::ChangeOutcome;
use listing_source_core::*;

use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

#[derive(Debug, Clone, PartialEq)]
pub enum RequiredPatch<T> {
    Unchanged,
    Set(T),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateListingSourceCommand {
    pub listing_source_id: ListingSourceId,
    pub name: RequiredPatch<ListingSourceName>,

    pub ingestion_configuration: RequiredPatch<ListingSourceIngestionConfigurations>,
    pub woocommerce_webhook_secret: PatchField<String>,
    pub url: PatchField<url::Url>,
    pub image: PatchField<url::Url>,
    pub referral_configuration: PatchField<ReferralConfiguration>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateListingSourceResult {
    pub listing_source_id: ListingSourceId,
    pub slug_id: ListingSourceSlugId,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateListingSourceError {
    #[error("authenticated actor required to update listing source")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    NotFound,
    #[error("operator party not found")]
    OperatorPartyNotFound,
    #[error("ingestion method/configuration mismatch")]
    ListingIngestionConfigurationMismatch,
    #[error("concurrent listing source update")]
    ConcurrencyConflict,
    #[error("listing source slug conflict")]
    SlugConflict {
        #[source]
        source: BoxError,
    },
    #[error("listing source Shopify domain conflict")]
    ShopifyDomainConflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary listing source persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted listing source state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal listing source failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin update listing source transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update listing source transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateListingSourceUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateListingSourceCommand,
    ) -> Result<UpdateListingSourceResult, UpdateListingSourceError>;
}

pub struct UpdateListingSourceHandler<U, S, A> {
    unit_of_work: U,
    sources: S,
    check_user_admin: A,
}

impl<U, S, A> UpdateListingSourceHandler<U, S, A> {
    pub fn new(unit_of_work: U, sources: S, check_user_admin: A) -> Self {
        Self {
            unit_of_work,
            sources,
            check_user_admin,
        }
    }
}

#[async_trait::async_trait]
impl<U, S, A> UpdateListingSourceUseCase for UpdateListingSourceHandler<U, S, A>
where
    U: UnitOfWork,
    S: ListingSourceRepositoryFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(
        name = "update_listing_source",
        skip_all,
        fields(
            listing_source_id = %command.listing_source_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateListingSourceCommand,
    ) -> Result<UpdateListingSourceResult, UpdateListingSourceError> {
        ensure_admin(context, &self.check_user_admin).await?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateListingSourceError::BeginTransactionFailed)?;
        let stored = self
            .sources
            .in_transaction(&mut tx)
            .find_by_id(command.listing_source_id)
            .await?
            .ok_or(UpdateListingSourceError::NotFound)?;
        let mut source = stored.source;
        let mut configuration = stored.configuration;
        let outcome = apply_update(&mut source, &mut configuration, &command)?;
        configuration
            .validate_for(&source)
            .map_err(|_| UpdateListingSourceError::ListingIngestionConfigurationMismatch)?;
        if command.woocommerce_webhook_secret.is_changed() && !configuration.has_woocommerce() {
            return Err(UpdateListingSourceError::ListingIngestionConfigurationMismatch);
        }
        let result = if outcome.changed() {
            self.sources
                .in_transaction(&mut tx)
                .update(
                    &source,
                    &configuration,
                    command.woocommerce_webhook_secret.as_str_patch(),
                    stored.version,
                )
                .await?
                .into()
        } else {
            UpdateListingSourceResult {
                listing_source_id: source.id(),
                slug_id: source.slug_id().clone(),
            }
        };
        tx.commit()
            .await
            .map_err(|_| UpdateListingSourceError::CommitTransactionFailed)?;
        tracing::info!(event = "listing_source.updated", actor_type = context.principal.kind(), actor_id = %context.principal.label(), listing_source_id = %result.listing_source_id, listing_source_slug_id = %result.slug_id, changed = outcome.changed(), outcome = "success");
        Ok(result)
    }
}

trait PatchFieldStrRef {
    fn as_str_patch(&self) -> PatchField<&str>;
}
impl PatchFieldStrRef for PatchField<String> {
    fn as_str_patch(&self) -> PatchField<&str> {
        match self {
            PatchField::Unchanged => PatchField::Unchanged,
            PatchField::Set(value) => PatchField::Set(value.as_str()),
            PatchField::Clear => PatchField::Clear,
        }
    }
}

impl From<StoredListingSource> for UpdateListingSourceResult {
    fn from(value: StoredListingSource) -> Self {
        Self {
            listing_source_id: value.source.id(),
            slug_id: value.source.slug_id().clone(),
        }
    }
}

fn apply_update(
    source: &mut ListingSource,
    configuration: &mut ListingSourceIngestionConfigurations,
    command: &UpdateListingSourceCommand,
) -> Result<ChangeOutcome, UpdateListingSourceError> {
    let mut outcome = ChangeOutcome::Unchanged;
    if let RequiredPatch::Set(name) = &command.name {
        outcome = outcome.combine(source.rename(name.clone()));
    }
    if let RequiredPatch::Set(value) = &command.ingestion_configuration {
        let methods = value
            .methods()
            .map_err(|_| UpdateListingSourceError::ListingIngestionConfigurationMismatch)?;
        outcome = outcome.combine(source.replace_ingestion_methods(methods));
        outcome = outcome.combine(ChangeOutcome::from(*configuration != *value));
        *configuration = value.clone();
    }
    if command.url.is_changed() || command.image.is_changed() {
        let presentation = ListingSourcePresentation {
            url: patch_url(source.presentation().url.clone(), &command.url),
            image: patch_url(source.presentation().image.clone(), &command.image),
        };
        outcome = outcome.combine(source.replace_presentation(presentation));
    }
    match &command.referral_configuration {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            outcome = outcome.combine(source.replace_referral_configuration(Some(value.clone())))
        }
        PatchField::Clear => outcome = outcome.combine(source.replace_referral_configuration(None)),
    }
    if command.woocommerce_webhook_secret.is_changed() && configuration.has_woocommerce() {
        outcome = outcome.combine(ChangeOutcome::Changed);
    }
    Ok(outcome)
}

fn patch_url(current: Option<url::Url>, patch: &PatchField<url::Url>) -> Option<url::Url> {
    match patch {
        PatchField::Unchanged => current,
        PatchField::Set(value) => Some(value.clone()),
        PatchField::Clear => None,
    }
}

async fn ensure_admin<A>(
    context: &OperationContext,
    check: &A,
) -> Result<(), UpdateListingSourceError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::Anonymous => Err(UpdateListingSourceError::AuthenticatedActorRequired),
        Principal::User(_) | Principal::DelegatedUser { .. } => check
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(|error| match error {
                CheckUserAdminError::AuthenticatedActorRequired => {
                    UpdateListingSourceError::AuthenticatedActorRequired
                }
                CheckUserAdminError::Forbidden => UpdateListingSourceError::Forbidden,
                CheckUserAdminError::TemporarilyUnavailable { source } => {
                    UpdateListingSourceError::TemporarilyUnavailable { source }
                }
                CheckUserAdminError::InvalidReadModel { source }
                | CheckUserAdminError::Internal { source } => {
                    UpdateListingSourceError::Internal { source }
                }
                CheckUserAdminError::BeginTransactionFailed
                | CheckUserAdminError::CommitTransactionFailed => {
                    UpdateListingSourceError::TemporarilyUnavailable {
                        source: static_error("check user admin transaction failed"),
                    }
                }
            }),
    }
}

impl From<ListingSourceRepositoryError> for UpdateListingSourceError {
    fn from(error: ListingSourceRepositoryError) -> Self {
        match error {
            ListingSourceRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ListingSourceRepositoryError::SlugConflict { source } => Self::SlugConflict { source },
            ListingSourceRepositoryError::ShopifyDomainConflict { source } => {
                Self::ShopifyDomainConflict { source }
            }
            ListingSourceRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ListingSourceRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ListingSourceRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        operation_context::{CorrelationId, RequestId},
        transaction::TransactionError,
    };
    use party_core::party_id::PartyId;

    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;
    use user_service::use_cases::queries::check_user_admin::CheckUserAdminResult;

    #[derive(Default)]
    struct State {
        updates: usize,

        commits: usize,
    }
    #[derive(Clone)]
    struct Uow(Arc<Mutex<State>>);
    struct Tx(Arc<Mutex<State>>);
    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            self.0
                .lock()
                .map_err(|_| TransactionError::CommitFailed)?
                .commits += 1;
            Ok(())
        }
    }
    #[async_trait::async_trait]
    impl UnitOfWork for Uow {
        type Tx = Tx;
        async fn begin(&self) -> Result<Tx, TransactionError> {
            Ok(Tx(Arc::clone(&self.0)))
        }
    }
    #[derive(Clone)]
    struct Sources(StoredListingSource);
    struct SourceRepo<'a>(&'a mut Tx, StoredListingSource);
    impl ListingSourceRepositoryFactory<Tx> for Sources {
        fn in_transaction<'a>(&'a self, tx: &'a mut Tx) -> impl ListingSourceRepository + 'a {
            SourceRepo(tx, self.0.clone())
        }
    }
    #[async_trait::async_trait]
    impl ListingSourceRepository for SourceRepo<'_> {
        async fn find_by_id(
            &mut self,
            _: ListingSourceId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(Some(self.1.clone()))
        }
        async fn find_by_slug(
            &mut self,
            _: &ListingSourceSlugId,
        ) -> Result<Option<StoredListingSource>, ListingSourceRepositoryError> {
            Ok(None)
        }
        async fn insert(
            &mut self,
            _: &ListingSource,
            _: &ListingSourceIngestionConfigurations,
            _: Option<&str>,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            Err(error())
        }
        async fn update(
            &mut self,
            source: &ListingSource,
            config: &ListingSourceIngestionConfigurations,
            _: PatchField<&str>,
            _: ListingSourceStorageVersion,
        ) -> Result<StoredListingSource, ListingSourceRepositoryError> {
            self.0.0.lock().map_err(|_| error())?.updates += 1;
            Ok(StoredListingSource {
                source: source.clone(),
                configuration: config.clone(),
                ..self.1.clone()
            })
        }
    }

    struct Admin;
    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for Admin {
        async fn execute(
            &self,
            _: &OperationContext,
            _: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            Ok(CheckUserAdminResult)
        }
    }
    fn source() -> StoredListingSource {
        let source = ListingSource::create(NewListingSource {
            id: ListingSourceId::new(),
            name: ListingSourceName::try_from("Source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
            operator_party_id: PartyId::new(),
            ingestion_methods: std::collections::HashSet::from([ListingIngestionMethod::WebCrawl]),
            presentation: ListingSourcePresentation::default(),
            referral_configuration: None,
        });
        StoredListingSource {
            source,
            configuration: ListingSourceIngestionConfigurations(vec![
                ListingIngestionConfiguration::WebCrawl {
                    fallback_currency: None,
                },
            ]),
            version: ListingSourceStorageVersion::INITIAL,
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn command(id: ListingSourceId) -> UpdateListingSourceCommand {
        UpdateListingSourceCommand {
            listing_source_id: id,
            name: RequiredPatch::Unchanged,

            ingestion_configuration: RequiredPatch::Unchanged,
            woocommerce_webhook_secret: PatchField::Unchanged,
            url: PatchField::Unchanged,
            image: PatchField::Unchanged,
            referral_configuration: PatchField::Unchanged,
        }
    }
    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }
    fn error() -> ListingSourceRepositoryError {
        ListingSourceRepositoryError::Internal {
            source: static_error("fake failure"),
        }
    }

    #[tokio::test]
    async fn should_persist_when_required_name_patch_is_set() {
        let stored = source();
        let state = Arc::new(Mutex::new(State::default()));
        let handler = UpdateListingSourceHandler::new(
            Uow(Arc::clone(&state)),
            Sources(stored.clone()),
            Admin,
        );
        let mut command = command(stored.source.id());
        command.name = RequiredPatch::Set(
            ListingSourceName::try_from("Renamed source")
                .unwrap_or_else(|error| panic!("invalid test listing source name: {error}")),
        );

        assert!(handler.execute(&context(), command).await.is_ok());
        let state = state
            .lock()
            .unwrap_or_else(|error| panic!("fake state poisoned: {error}"));
        assert_eq!(1, state.updates);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_skip_persistence_for_no_op_patch() {
        let stored = source();
        let state = Arc::new(Mutex::new(State::default()));
        let handler = UpdateListingSourceHandler::new(
            Uow(Arc::clone(&state)),
            Sources(stored.clone()),
            Admin,
        );
        assert!(
            handler
                .execute(&context(), command(stored.source.id()))
                .await
                .is_ok()
        );
        let state = state
            .lock()
            .unwrap_or_else(|error| panic!("fake state poisoned: {error}"));
        assert_eq!(0, state.updates);

        assert_eq!(1, state.commits);
    }
}

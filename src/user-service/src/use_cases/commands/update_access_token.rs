use crate::ports::{
    AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
};
use application::error::BoxError;
use application::operation_context::{
    AuthenticationRequired, CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessToken, AccessTokenId, AccessTokenName, Scope};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateAccessTokenCommand {
    pub user_id: UserId,
    pub access_token_id: AccessTokenId,
    pub name: PatchField<AccessTokenName>,
    pub scopes: PatchField<HashSet<Scope>>,
    pub expires: PatchField<OffsetDateTime>,
}

impl UpdateAccessTokenCommand {
    pub fn is_empty(&self) -> bool {
        !self.name.is_changed() && !self.scopes.is_changed() && !self.expires.is_changed()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAccessTokenResult {
    pub view: crate::use_cases::queries::get_access_token::AccessTokenView,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateAccessTokenError {
    #[error("authenticated actor required to update access token")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("access token not found")]
    AccessTokenNotFound,
    #[error("access token name is required")]
    NameRequired,
    #[error("concurrent access token update")]
    ConcurrencyConflict,
    #[error("access token already exists")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary access token store failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted access token state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal access token persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin update access token transaction")]
    BeginTransactionFailed,
    #[error("failed to commit update access token transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateAccessTokenUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAccessTokenCommand,
    ) -> Result<UpdateAccessTokenResult, UpdateAccessTokenError>;
}

pub struct UpdateAccessTokenHandler<U, R> {
    unit_of_work: U,
    repository: R,
}

impl<U, R> UpdateAccessTokenHandler<U, R> {
    pub fn new(unit_of_work: U, repository: R) -> Self {
        Self {
            unit_of_work,
            repository,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> UpdateAccessTokenUseCase for UpdateAccessTokenHandler<U, R>
where
    U: UnitOfWork,
    R: AccessTokenRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_access_token",
        skip_all,
        fields(
            user_id = %command.user_id,
            access_token_id = %command.access_token_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateAccessTokenCommand,
    ) -> Result<UpdateAccessTokenResult, UpdateAccessTokenError> {
        authorize_access_token_write(context, command.user_id)?;
        let principal = context.principal.require_authenticated()?.clone();
        tracing::Span::current().record("actor_id", tracing::field::display(principal.label()));

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateAccessTokenError::BeginTransactionFailed)?;
        let mut repository = self.repository.in_transaction(&mut tx);
        let domain_primitives::versioned::Versioned {
            value: mut access_token,
            version,
        } = repository
            .find_by_id(command.user_id, command.access_token_id)
            .await?
            .ok_or(UpdateAccessTokenError::AccessTokenNotFound)?;

        let changed = apply_update(&mut access_token, command)?;
        if changed {
            access_token = repository.update(&access_token, version).await?.value;
        }
        drop(repository);
        tx.commit()
            .await
            .map_err(|_| UpdateAccessTokenError::CommitTransactionFailed)?;

        tracing::info!(
            event = "access_token.updated",
            actor_id = %principal.label(),
            user_id = %access_token.user_id(),
            access_token_id = %access_token.id(),
            changed,
            outcome = "success",
        );

        Ok(UpdateAccessTokenResult {
            view: crate::use_cases::queries::get_access_token::AccessTokenView::from(access_token),
        })
    }
}

fn apply_update(
    access_token: &mut AccessToken,
    command: UpdateAccessTokenCommand,
) -> Result<bool, UpdateAccessTokenError> {
    let mut changed = false;

    match command.name {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.change_name(value);
        }
        PatchField::Clear => return Err(UpdateAccessTokenError::NameRequired),
    }
    match command.scopes {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.replace_scopes(value);
        }
        PatchField::Clear => {
            changed |= access_token.replace_scopes(HashSet::new());
        }
    }
    match command.expires {
        PatchField::Unchanged => {}
        PatchField::Set(value) => {
            changed |= access_token.change_expires(Some(value));
        }
        PatchField::Clear => {
            changed |= access_token.change_expires(None);
        }
    }

    Ok(changed)
}

impl From<OperationAuthorizationError> for UpdateAccessTokenError {
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

impl From<AuthenticationRequired> for UpdateAccessTokenError {
    fn from(_: AuthenticationRequired) -> Self {
        Self::AuthenticatedActorRequired
    }
}

fn authorize_access_token_write(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UpdateAccessTokenError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UpdateAccessTokenError>()
}

impl From<AccessTokenRepositoryError> for UpdateAccessTokenError {
    fn from(error: AccessTokenRepositoryError) -> Self {
        match error {
            AccessTokenRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            AccessTokenRepositoryError::Conflict { source } => Self::Conflict { source },
            AccessTokenRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AccessTokenRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            AccessTokenRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        UpdateAccessTokenCommand, UpdateAccessTokenError, UpdateAccessTokenHandler,
        UpdateAccessTokenUseCase,
    };
    use crate::ports::{
        AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
        AccessTokenStorageVersion, VersionedAccessToken,
    };
    use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
    use application::patch_field::PatchField;
    use application::transaction::{Transaction, TransactionError, UnitOfWork};
    use domain_primitives::versioned::Versioned;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, MutexGuard};
    use user_core::access_token::{
        AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, HashedRawAccessToken,
        NewAccessToken, RawAccessToken, Scope,
    };
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        token: Option<VersionedAccessToken>,
        begins: usize,
        commits: usize,
        find_calls: usize,
        update_calls: usize,
    }

    #[derive(Clone, Default)]
    struct Fakes(Arc<Mutex<State>>);
    struct FakeTx(Fakes);
    struct FakeRepository(Fakes);

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("req-test"),
            correlation_id: CorrelationId::new("corr-test"),
        }
    }

    fn token(user_id: UserId) -> AccessToken {
        AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: RawAccessToken::new().into(),
            user_id,
            name: AccessTokenName::from("test token"),
            scopes: HashSet::from([Scope::ProductListingsWrite]),
            origin: AccessTokenOrigin::User,
            expires: None,
        })
    }

    fn versioned(token: AccessToken) -> VersionedAccessToken {
        Versioned::new(token, AccessTokenStorageVersion::INITIAL)
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0.0).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Fakes {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            lock(&self.0).begins += 1;
            Ok(FakeTx(self.clone()))
        }
    }

    #[async_trait::async_trait]
    impl AccessTokenRepository for FakeRepository {
        async fn find_by_id(
            &mut self,
            user_id: UserId,
            access_token_id: AccessTokenId,
        ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
            let mut state = lock(&self.0.0);
            state.find_calls += 1;
            Ok(state.token.clone().filter(|token| {
                token.value.user_id() == user_id && token.value.id() == access_token_id
            }))
        }

        async fn find_by_hashed_token(
            &mut self,
            _hashed_token: &HashedRawAccessToken,
        ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
            Ok(None)
        }

        async fn insert(
            &mut self,
            _token: &AccessToken,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            Err(AccessTokenRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other("not used")),
            })
        }

        async fn update(
            &mut self,
            token: &AccessToken,
            _expected_version: AccessTokenStorageVersion,
        ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
            let persisted = versioned(token.clone());
            let mut state = lock(&self.0.0);
            state.update_calls += 1;
            state.token = Some(persisted.clone());
            Ok(persisted)
        }

        async fn delete_by_id(
            &mut self,
            _user_id: UserId,
            _access_token_id: AccessTokenId,
        ) -> Result<bool, AccessTokenRepositoryError> {
            Ok(true)
        }

        async fn delete_by_user_id(
            &mut self,
            _user_id: UserId,
        ) -> Result<u64, AccessTokenRepositoryError> {
            Ok(0)
        }
    }

    impl AccessTokenRepositoryFactory<FakeTx> for Fakes {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl AccessTokenRepository + 'tx {
            FakeRepository(self.clone())
        }
    }

    #[test]
    fn should_report_empty_update_when_all_fields_unchanged() {
        assert!(
            UpdateAccessTokenCommand {
                user_id: UserId::new(),
                access_token_id: AccessTokenId::new(),
                ..Default::default()
            }
            .is_empty()
        );
    }

    #[tokio::test]
    async fn should_update_access_token_once_and_skip_noop_persistence() {
        let user_id = UserId::new();
        let current = token(user_id);
        let fakes = Fakes::default();
        lock(&fakes.0).token = Some(versioned(current.clone()));

        let changed = UpdateAccessTokenHandler::new(fakes.clone(), fakes.clone())
            .execute(
                &context(Principal::User(user_id)),
                UpdateAccessTokenCommand {
                    user_id,
                    access_token_id: current.id(),
                    name: PatchField::Set(AccessTokenName::from("updated")),
                    ..Default::default()
                },
            )
            .await;
        assert!(changed.is_ok());

        let unchanged = UpdateAccessTokenHandler::new(fakes.clone(), fakes.clone())
            .execute(
                &context(Principal::System),
                UpdateAccessTokenCommand {
                    user_id,
                    access_token_id: current.id(),
                    ..Default::default()
                },
            )
            .await;
        assert!(unchanged.is_ok());

        let state = lock(&fakes.0);
        assert_eq!(2, state.begins);
        assert_eq!(2, state.find_calls);
        assert_eq!(1, state.update_calls);
        assert_eq!(2, state.commits);
    }

    #[tokio::test]
    async fn should_return_not_found_after_starting_update_transaction() {
        let fakes = Fakes::default();
        let result = UpdateAccessTokenHandler::new(fakes.clone(), fakes.clone())
            .execute(
                &context(Principal::System),
                UpdateAccessTokenCommand {
                    user_id: UserId::new(),
                    access_token_id: AccessTokenId::new(),
                    ..Default::default()
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(UpdateAccessTokenError::AccessTokenNotFound)
        ));
        let state = lock(&fakes.0);
        assert_eq!(1, state.begins);
        assert_eq!(0, state.commits);
    }
}

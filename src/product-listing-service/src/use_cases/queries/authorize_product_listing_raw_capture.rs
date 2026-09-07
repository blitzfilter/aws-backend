use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory,
};
use application::{
    error::{BoxError, box_error},
    operation_context::{
        CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
    },
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizeProductListingRawCaptureRequest {
    pub listing_source_id: ListingSourceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizeProductListingRawCaptureResult;

#[derive(Debug, thiserror::Error)]
pub enum AuthorizeProductListingRawCaptureError {
    #[error("authenticated actor required to authorize raw product listing capture")]
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
    #[error("failed to begin raw product listing capture authorization transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit raw product listing capture authorization transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait AuthorizeProductListingRawCaptureUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthorizeProductListingRawCaptureRequest,
    ) -> Result<AuthorizeProductListingRawCaptureResult, AuthorizeProductListingRawCaptureError>;
}

pub struct AuthorizeProductListingRawCaptureHandler<U, A> {
    unit_of_work: U,
    authorizer: A,
}

impl<U, A> AuthorizeProductListingRawCaptureHandler<U, A> {
    pub fn new(unit_of_work: U, authorizer: A) -> Self {
        Self {
            unit_of_work,
            authorizer,
        }
    }
}

#[async_trait::async_trait]
impl<U, A> AuthorizeProductListingRawCaptureUseCase
    for AuthorizeProductListingRawCaptureHandler<U, A>
where
    U: UnitOfWork,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "authorize_product_listing_raw_capture",
        skip_all,
        fields(
            listing_source_id = %request.listing_source_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: AuthorizeProductListingRawCaptureRequest,
    ) -> Result<AuthorizeProductListingRawCaptureResult, AuthorizeProductListingRawCaptureError>
    {
        let actor_id = authorized_actor(context)?;
        tracing::Span::current().record("actor_id", tracing::field::display(actor_id));

        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            AuthorizeProductListingRawCaptureError::BeginTransactionFailed {
                source: box_error(source),
            }
        })?;
        self.authorizer
            .in_transaction(&mut tx)
            .authorize(actor_id, request.listing_source_id)
            .await?;
        tx.commit().await.map_err(|source| {
            AuthorizeProductListingRawCaptureError::CommitTransactionFailed {
                source: box_error(source),
            }
        })?;

        Ok(AuthorizeProductListingRawCaptureResult)
    }
}

fn authorized_actor(
    context: &OperationContext,
) -> Result<UserId, AuthorizeProductListingRawCaptureError> {
    context
        .require()
        .credential_capability(CredentialCapability::ProductListingsWrite)
        .any_user()
        .authorize::<AuthorizeProductListingRawCaptureError>()?;

    match &context.principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Ok(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => {
            Err(AuthorizeProductListingRawCaptureError::Forbidden)
        }
    }
}

impl From<OperationAuthorizationError> for AuthorizeProductListingRawCaptureError {
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

impl From<PartnerProductListingAuthorizationError> for AuthorizeProductListingRawCaptureError {
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

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, RequestId},
        transaction::TransactionError,
    };
    use std::{
        collections::BTreeSet,
        error::Error,
        sync::{Arc, Mutex, MutexGuard},
    };

    #[derive(Default)]
    struct State {
        begin_fails: bool,
        commit_fails: bool,
        authorization_error: Option<PartnerProductListingAuthorizationError>,
        begins: usize,
        authorization_calls: usize,
        authorizations: Vec<(UserId, ListingSourceId)>,
        commit_attempts: usize,
        commits: usize,
    }

    type SharedState = Arc<Mutex<State>>;

    #[derive(Clone)]
    struct UnitOfWorkFake(SharedState);

    struct TransactionFake(SharedState);

    #[derive(Clone)]
    struct AuthorizerFactoryFake(SharedState);

    struct AuthorizerFake(SharedState);

    fn state() -> SharedState {
        Arc::new(Mutex::new(State::default()))
    }

    fn lock(state: &SharedState) -> MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn handler(
        state: &SharedState,
    ) -> AuthorizeProductListingRawCaptureHandler<UnitOfWorkFake, AuthorizerFactoryFake> {
        AuthorizeProductListingRawCaptureHandler::new(
            UnitOfWorkFake(Arc::clone(state)),
            AuthorizerFactoryFake(Arc::clone(state)),
        )
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(listing_source_id: ListingSourceId) -> AuthorizeProductListingRawCaptureRequest {
        AuthorizeProductListingRawCaptureRequest { listing_source_id }
    }

    fn delegated_context(
        user_id: UserId,
        capabilities: BTreeSet<CredentialCapability>,
    ) -> OperationContext {
        context(Principal::DelegatedUser {
            user_id,
            capabilities,
        })
    }

    fn assert_source(error: &AuthorizeProductListingRawCaptureError) {
        assert!(
            Error::source(error).is_some(),
            "error source must be retained"
        );
    }

    #[async_trait::async_trait]
    impl UnitOfWork for UnitOfWorkFake {
        type Tx = TransactionFake;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            state.begins += 1;
            if state.begin_fails {
                return Err(TransactionError::BeginFailed);
            }
            Ok(TransactionFake(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl Transaction for TransactionFake {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.0);
            state.commit_attempts += 1;
            if state.commit_fails {
                return Err(TransactionError::CommitFailed);
            }
            state.commits += 1;
            Ok(())
        }
    }

    impl PartnerProductListingAuthorizerFactory<TransactionFake> for AuthorizerFactoryFake {
        fn in_transaction<'tx>(
            &'tx self,
            _: &'tx mut TransactionFake,
        ) -> impl PartnerProductListingAuthorizer + 'tx {
            AuthorizerFake(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl PartnerProductListingAuthorizer for AuthorizerFake {
        async fn authorize(
            &mut self,
            actor_id: UserId,
            listing_source_id: ListingSourceId,
        ) -> Result<(), PartnerProductListingAuthorizationError> {
            let mut state = lock(&self.0);
            state.authorization_calls += 1;
            state.authorizations.push((actor_id, listing_source_id));
            if let Some(error) = state.authorization_error.take() {
                return Err(error);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn should_authorize_exact_source_and_commit_for_delegated_actor_with_capability() {
        let state = state();
        let user_id = UserId::new();
        let listing_source_id = ListingSourceId::new();

        let result = handler(&state)
            .execute(
                &delegated_context(
                    user_id,
                    BTreeSet::from([CredentialCapability::ProductListingsWrite]),
                ),
                request(listing_source_id),
            )
            .await;

        assert!(matches!(
            result,
            Ok(AuthorizeProductListingRawCaptureResult)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.authorization_calls);
        assert_eq!(vec![(user_id, listing_source_id)], state.authorizations);
        assert_eq!(1, state.commit_attempts);
        assert_eq!(1, state.commits);
    }

    #[tokio::test]
    async fn should_reject_delegated_actor_without_product_listings_write_before_transaction() {
        let state = state();

        let result = handler(&state)
            .execute(
                &delegated_context(UserId::new(), BTreeSet::new()),
                request(ListingSourceId::new()),
            )
            .await;

        assert!(matches!(
            result,
            Err(AuthorizeProductListingRawCaptureError::Forbidden)
        ));
        let state = lock(&state);
        assert_eq!(0, state.begins);
        assert_eq!(0, state.authorization_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_reject_anonymous_actor_before_transaction() {
        let state = state();

        let result = handler(&state)
            .execute(
                &context(Principal::Anonymous),
                request(ListingSourceId::new()),
            )
            .await;

        assert!(matches!(
            result,
            Err(AuthorizeProductListingRawCaptureError::AuthenticatedActorRequired)
        ));
        let state = lock(&state);
        assert_eq!(0, state.begins);
        assert_eq!(0, state.authorization_calls);
        assert_eq!(0, state.commit_attempts);
    }

    #[tokio::test]
    async fn should_reject_denied_source_grant_without_commit() {
        let state = state();
        let user_id = UserId::new();
        let listing_source_id = ListingSourceId::new();
        lock(&state).authorization_error = Some(PartnerProductListingAuthorizationError::Forbidden);

        let result = handler(&state)
            .execute(
                &delegated_context(
                    user_id,
                    BTreeSet::from([CredentialCapability::ProductListingsWrite]),
                ),
                request(listing_source_id),
            )
            .await;

        assert!(matches!(
            result,
            Err(AuthorizeProductListingRawCaptureError::Forbidden)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.authorization_calls);
        assert_eq!(vec![(user_id, listing_source_id)], state.authorizations);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_not_commit_when_listing_source_is_missing() {
        let state = state();
        let user_id = UserId::new();
        let listing_source_id = ListingSourceId::new();
        lock(&state).authorization_error =
            Some(PartnerProductListingAuthorizationError::ListingSourceNotFound);

        let result = handler(&state)
            .execute(
                &delegated_context(
                    user_id,
                    BTreeSet::from([CredentialCapability::ProductListingsWrite]),
                ),
                request(listing_source_id),
            )
            .await;

        assert!(matches!(
            result,
            Err(AuthorizeProductListingRawCaptureError::ListingSourceNotFound)
        ));
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.authorization_calls);
        assert_eq!(vec![(user_id, listing_source_id)], state.authorizations);
        assert_eq!(0, state.commit_attempts);
        assert_eq!(0, state.commits);
    }

    #[test]
    fn should_map_partner_authorization_errors_to_stable_errors_with_sources() {
        assert!(matches!(
            AuthorizeProductListingRawCaptureError::from(
                PartnerProductListingAuthorizationError::ListingSourceNotFound
            ),
            AuthorizeProductListingRawCaptureError::ListingSourceNotFound
        ));

        let temporarily_unavailable = AuthorizeProductListingRawCaptureError::from(
            PartnerProductListingAuthorizationError::TemporarilyUnavailable {
                source: static_error("authorizer temporarily unavailable"),
            },
        );
        assert!(matches!(
            temporarily_unavailable,
            AuthorizeProductListingRawCaptureError::PartnerAuthorizationTemporarilyUnavailable { .. }
        ));
        assert_source(&temporarily_unavailable);

        let internal = AuthorizeProductListingRawCaptureError::from(
            PartnerProductListingAuthorizationError::Internal {
                source: static_error("authorizer internal failure"),
            },
        );
        assert!(matches!(
            internal,
            AuthorizeProductListingRawCaptureError::PartnerAuthorizationInternal { .. }
        ));
        assert_source(&internal);
    }

    #[tokio::test]
    async fn should_preserve_transaction_failure_sources() {
        let begin_state = state();
        lock(&begin_state).begin_fails = true;
        let begin_error = match handler(&begin_state)
            .execute(
                &delegated_context(
                    UserId::new(),
                    BTreeSet::from([CredentialCapability::ProductListingsWrite]),
                ),
                request(ListingSourceId::new()),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("begin failure must return an error"),
        };
        assert!(matches!(
            begin_error,
            AuthorizeProductListingRawCaptureError::BeginTransactionFailed { .. }
        ));
        assert_source(&begin_error);
        {
            let begin_snapshot = lock(&begin_state);
            assert_eq!(1, begin_snapshot.begins);
            assert_eq!(0, begin_snapshot.authorization_calls);
            assert_eq!(0, begin_snapshot.commit_attempts);
        }

        let commit_state = state();
        lock(&commit_state).commit_fails = true;
        let commit_error = match handler(&commit_state)
            .execute(
                &delegated_context(
                    UserId::new(),
                    BTreeSet::from([CredentialCapability::ProductListingsWrite]),
                ),
                request(ListingSourceId::new()),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("commit failure must return an error"),
        };
        assert!(matches!(
            commit_error,
            AuthorizeProductListingRawCaptureError::CommitTransactionFailed { .. }
        ));
        assert_source(&commit_error);
        let commit_state = lock(&commit_state);
        assert_eq!(1, commit_state.begins);
        assert_eq!(1, commit_state.authorization_calls);
        assert_eq!(1, commit_state.commit_attempts);
        assert_eq!(0, commit_state.commits);
    }
}

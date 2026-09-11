use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use listing_source_core::ListingSourceSlugId;

use crate::{
    ports::{
        PublicListingSourceDetailsReadError, PublicListingSourceDetailsReader,
        PublicListingSourceDetailsReaderFactory,
    },
    use_cases::queries::public_listing_source::PublicListingSourceSummary,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetPublicListingSourceBySlugRequest {
    pub slug_id: ListingSourceSlugId,
}

pub type GetPublicListingSourceBySlugResult = PublicListingSourceSummary;

#[derive(Debug, thiserror::Error)]
pub enum GetPublicListingSourceBySlugError {
    #[error("listing source not found")]
    NotFound,
    #[error("temporary public listing source detail read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public listing source detail read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal public listing source detail read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin public listing source detail transaction")]
    BeginTransaction {
        #[source]
        source: TransactionError,
    },
    #[error("failed to commit public listing source detail transaction")]
    CommitTransaction {
        #[source]
        source: TransactionError,
    },
}

#[async_trait::async_trait]
pub trait GetPublicListingSourceBySlugUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPublicListingSourceBySlugRequest,
    ) -> Result<GetPublicListingSourceBySlugResult, GetPublicListingSourceBySlugError>;
}

pub struct GetPublicListingSourceBySlugHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> GetPublicListingSourceBySlugHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> GetPublicListingSourceBySlugUseCase for GetPublicListingSourceBySlugHandler<U, R>
where
    U: UnitOfWork,
    R: PublicListingSourceDetailsReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_public_listing_source_by_slug",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPublicListingSourceBySlugRequest,
    ) -> Result<GetPublicListingSourceBySlugResult, GetPublicListingSourceBySlugError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|source| GetPublicListingSourceBySlugError::BeginTransaction { source })?;
        let summary = self
            .reader
            .in_transaction(&mut tx)
            .find_by_slug(&request.slug_id)
            .await?;
        tx.commit()
            .await
            .map_err(|source| GetPublicListingSourceBySlugError::CommitTransaction { source })?;

        summary.ok_or(GetPublicListingSourceBySlugError::NotFound)
    }
}

impl From<PublicListingSourceDetailsReadError> for GetPublicListingSourceBySlugError {
    fn from(value: PublicListingSourceDetailsReadError) -> Self {
        match value {
            PublicListingSourceDetailsReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PublicListingSourceDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            PublicListingSourceDetailsReadError::Internal { source } => Self::Internal { source },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ports::{PublicListingSourceDetailsReader, PublicListingSourceDetailsReaderFactory},
        use_cases::queries::public_listing_source::{
            PublicListingSourceOperatorSummary, PublicListingSourceSummary,
        },
    };
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
    };
    use listing_source_core::{ListingSourceId, ListingSourceName};
    use party_core::party_name::PartyName;
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex, MutexGuard},
    };
    use user_core::user_id::UserId;

    #[derive(Default)]
    struct State {
        begins: usize,
        bindings: usize,
        reads: usize,
        commits: usize,
    }

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: Arc<Mutex<State>>,
        begin_fails: bool,
        commit_fails: bool,
    }

    struct FakeTransaction {
        state: Arc<Mutex<State>>,
        commit_fails: bool,
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            if self.commit_fails {
                return Err(TransactionError::CommitFailed);
            }
            lock(&self.state).commits += 1;
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if self.begin_fails {
                return Err(TransactionError::BeginFailed);
            }
            lock(&self.state).begins += 1;
            Ok(FakeTransaction {
                state: Arc::clone(&self.state),
                commit_fails: self.commit_fails,
            })
        }
    }

    #[derive(Clone, Copy)]
    enum ReaderOutcome {
        Missing,
        TemporarilyUnavailable,
        InvalidReadModel,
        Internal,
    }

    #[derive(Clone)]
    struct FakeReaderFactory {
        state: Arc<Mutex<State>>,
        summary: PublicListingSourceSummary,
        outcome: Option<ReaderOutcome>,
    }

    struct FakeReader {
        state: Arc<Mutex<State>>,
        summary: PublicListingSourceSummary,
        outcome: Option<ReaderOutcome>,
    }

    impl PublicListingSourceDetailsReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PublicListingSourceDetailsReader + 'tx {
            lock(&self.state).bindings += 1;
            FakeReader {
                state: Arc::clone(&self.state),
                summary: self.summary.clone(),
                outcome: self.outcome,
            }
        }
    }

    #[async_trait::async_trait]
    impl PublicListingSourceDetailsReader for FakeReader {
        async fn find_by_slug(
            &mut self,
            _slug_id: &ListingSourceSlugId,
        ) -> Result<Option<PublicListingSourceSummary>, PublicListingSourceDetailsReadError>
        {
            lock(&self.state).reads += 1;
            match self.outcome {
                None => Ok(Some(self.summary.clone())),
                Some(ReaderOutcome::Missing) => Ok(None),
                Some(ReaderOutcome::TemporarilyUnavailable) => Err(
                    PublicListingSourceDetailsReadError::TemporarilyUnavailable {
                        source: static_error("reader unavailable"),
                    },
                ),
                Some(ReaderOutcome::InvalidReadModel) => {
                    Err(PublicListingSourceDetailsReadError::InvalidReadModel {
                        source: static_error("invalid row"),
                    })
                }
                Some(ReaderOutcome::Internal) => {
                    Err(PublicListingSourceDetailsReadError::Internal {
                        source: static_error("reader failed"),
                    })
                }
            }
        }
    }

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        match value.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request() -> Result<GetPublicListingSourceBySlugRequest, Box<dyn std::error::Error>> {
        Ok(GetPublicListingSourceBySlugRequest {
            slug_id: ListingSourceSlugId::raw("source")?,
        })
    }

    fn summary() -> Result<PublicListingSourceSummary, Box<dyn std::error::Error>> {
        Ok(PublicListingSourceSummary {
            listing_source_id: ListingSourceId::new(),
            listing_source_slug_id: ListingSourceSlugId::raw("source")?,
            name: ListingSourceName::try_from("Source")?,
            operator: PublicListingSourceOperatorSummary {
                name: PartyName::try_from("Operator")?,
            },
            url: None,
            image: None,
        })
    }

    fn handler(
        state: Arc<Mutex<State>>,
        outcome: Option<ReaderOutcome>,
        begin_fails: bool,
        commit_fails: bool,
    ) -> Result<
        GetPublicListingSourceBySlugHandler<FakeUnitOfWork, FakeReaderFactory>,
        Box<dyn std::error::Error>,
    > {
        Ok(GetPublicListingSourceBySlugHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
                begin_fails,
                commit_fails,
            },
            FakeReaderFactory {
                state,
                summary: summary()?,
                outcome,
            },
        ))
    }

    #[tokio::test]
    async fn should_allow_anonymous_and_delegated_callers_without_extra_authorization()
    -> Result<(), Box<dyn std::error::Error>> {
        for principal in [
            Principal::Anonymous,
            Principal::User(UserId::new()),
            Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: BTreeSet::new(),
            },
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let handler = handler(Arc::clone(&state), None, false, false)?;

            let result = handler.execute(&context(principal), request()?).await;

            assert!(result.is_ok());
            let state = lock(&state);
            assert_eq!(1, state.begins);
            assert_eq!(1, state.bindings);
            assert_eq!(1, state.reads);
            assert_eq!(1, state.commits);
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_map_missing_detail_to_not_found_after_committing_successful_read()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = handler(
            Arc::clone(&state),
            Some(ReaderOutcome::Missing),
            false,
            false,
        )?;

        let result = handler
            .execute(&context(Principal::Anonymous), request()?)
            .await;

        assert!(matches!(
            result,
            Err(GetPublicListingSourceBySlugError::NotFound)
        ));
        assert_eq!(1, lock(&state).reads);
        assert_eq!(1, lock(&state).commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_reader_failures_without_mapping_to_not_found()
    -> Result<(), Box<dyn std::error::Error>> {
        for outcome in [
            ReaderOutcome::TemporarilyUnavailable,
            ReaderOutcome::InvalidReadModel,
            ReaderOutcome::Internal,
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let handler = handler(Arc::clone(&state), Some(outcome), false, false)?;

            let result = handler
                .execute(&context(Principal::Anonymous), request()?)
                .await;

            assert!(matches!(
                result,
                Err(GetPublicListingSourceBySlugError::TemporarilyUnavailable { .. })
                    | Err(GetPublicListingSourceBySlugError::InvalidReadModel { .. })
                    | Err(GetPublicListingSourceBySlugError::Internal { .. })
            ));
            assert_eq!(0, lock(&state).commits);
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_map_transaction_failures() -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let begin_failure = handler(Arc::clone(&state), None, true, false)?
            .execute(&context(Principal::Anonymous), request()?)
            .await;
        assert!(matches!(
            begin_failure,
            Err(GetPublicListingSourceBySlugError::BeginTransaction { .. })
        ));
        assert_eq!(0, lock(&state).reads);

        let state = Arc::new(Mutex::new(State::default()));
        let commit_failure = handler(Arc::clone(&state), None, false, true)?
            .execute(&context(Principal::Anonymous), request()?)
            .await;
        assert!(matches!(
            commit_failure,
            Err(GetPublicListingSourceBySlugError::CommitTransaction { .. })
        ));
        assert_eq!(1, lock(&state).reads);
        assert_eq!(0, lock(&state).commits);
        Ok(())
    }
}

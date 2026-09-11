use application::{
    error::BoxError,
    operation_context::OperationContext,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::{
    ports::{
        PublicListingSourceSearchReadError, PublicListingSourceSearchReader,
        PublicListingSourceSearchReaderFactory,
    },
    use_cases::queries::public_listing_source::PublicListingSourceSummary,
};

pub const DEFAULT_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE: u8 = 21;
pub const MAX_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE: u8 = 50;
pub const MAX_PUBLIC_LISTING_SOURCE_SEARCH_QUERY_UTF8_BYTES: usize = 1_024;
pub const MAX_PUBLIC_LISTING_SOURCE_SEARCH_QUERY_SCALARS: usize = 256;
pub const MAX_PUBLIC_LISTING_SOURCE_SEARCH_POSITION_NAME_UTF8_BYTES: usize = 2_048;

const ENDPOINT_IDENTITY: &[u8] = b"GET /api/v1/listing-sources";
const SORT_IDENTITY: &[u8] = b"match_tier:name_search:C:listing_source_id";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceSearchQuery {
    state: PublicListingSourceSearchQueryState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PublicListingSourceSearchQueryState {
    Browse,
    InsufficientInput,
    Text(String),
}

impl PublicListingSourceSearchQuery {
    pub fn new(value: Option<String>) -> Result<Self, PublicListingSourceSearchQueryError> {
        let Some(value) = value else {
            return Ok(Self {
                state: PublicListingSourceSearchQueryState::Browse,
            });
        };

        validate_query_limits(&value)?;
        reject_non_whitespace_controls(&value)?;

        let canonical = canonicalize_query(&value);

        validate_query_limits(&canonical)?;
        reject_non_whitespace_controls(&canonical)?;

        if canonical.is_empty() {
            return Ok(Self {
                state: PublicListingSourceSearchQueryState::Browse,
            });
        }

        if canonical.chars().count() == 1 || !canonical.chars().any(char::is_alphanumeric) {
            return Ok(Self {
                state: PublicListingSourceSearchQueryState::InsufficientInput,
            });
        }

        Ok(Self {
            state: PublicListingSourceSearchQueryState::Text(canonical),
        })
    }

    pub fn canonical_text(&self) -> Option<&str> {
        match &self.state {
            PublicListingSourceSearchQueryState::Text(value) => Some(value),
            PublicListingSourceSearchQueryState::Browse
            | PublicListingSourceSearchQueryState::InsufficientInput => None,
        }
    }

    pub const fn is_browse(&self) -> bool {
        matches!(self.state, PublicListingSourceSearchQueryState::Browse)
    }

    pub const fn is_insufficient_input(&self) -> bool {
        matches!(
            self.state,
            PublicListingSourceSearchQueryState::InsufficientInput
        )
    }

    fn mode_identity(&self) -> &'static [u8] {
        match &self.state {
            PublicListingSourceSearchQueryState::Browse => b"browse",
            PublicListingSourceSearchQueryState::InsufficientInput => b"insufficient-input",
            PublicListingSourceSearchQueryState::Text(_) => b"text",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicListingSourceSearchQueryError {
    #[error("public listing source search query exceeds {max_bytes} UTF-8 bytes")]
    TooManyUtf8Bytes { max_bytes: usize },
    #[error("public listing source search query exceeds {max_scalars} Unicode scalar values")]
    TooManyUnicodeScalars { max_scalars: usize },
    #[error("public listing source search query contains a non-whitespace control character")]
    NonWhitespaceControlCharacter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceSearchPosition {
    match_tier: u8,
    name_search: String,
    listing_source_id: ListingSourceId,
}

impl PublicListingSourceSearchPosition {
    pub fn new(
        match_tier: u8,
        name_search: String,
        listing_source_id: ListingSourceId,
    ) -> Result<Self, PublicListingSourceSearchPositionError> {
        if match_tier > 5 {
            return Err(PublicListingSourceSearchPositionError::InvalidMatchTier { match_tier });
        }
        if name_search.is_empty() {
            return Err(PublicListingSourceSearchPositionError::EmptyNameSearch);
        }
        if name_search.len() > MAX_PUBLIC_LISTING_SOURCE_SEARCH_POSITION_NAME_UTF8_BYTES {
            return Err(PublicListingSourceSearchPositionError::NameSearchTooLong {
                max_bytes: MAX_PUBLIC_LISTING_SOURCE_SEARCH_POSITION_NAME_UTF8_BYTES,
            });
        }

        Ok(Self {
            match_tier,
            name_search,
            listing_source_id,
        })
    }

    pub const fn match_tier(&self) -> u8 {
        self.match_tier
    }

    pub fn name_search(&self) -> &str {
        &self.name_search
    }

    pub const fn listing_source_id(&self) -> ListingSourceId {
        self.listing_source_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicListingSourceSearchPositionError {
    #[error("public listing source search match tier {match_tier} is invalid")]
    InvalidMatchTier { match_tier: u8 },
    #[error("public listing source search position name is empty")]
    EmptyNameSearch,
    #[error("public listing source search position name exceeds {max_bytes} UTF-8 bytes")]
    NameSearchTooLong { max_bytes: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceSearchContinuation {
    binding: [u8; 32],
    position: PublicListingSourceSearchPosition,
}

impl PublicListingSourceSearchContinuation {
    pub fn new(binding: [u8; 32], position: PublicListingSourceSearchPosition) -> Self {
        Self { binding, position }
    }

    pub const fn binding(&self) -> &[u8; 32] {
        &self.binding
    }

    pub const fn position(&self) -> &PublicListingSourceSearchPosition {
        &self.position
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPublicListingSourcesRequest {
    query: PublicListingSourceSearchQuery,
    page_size: u8,
    continuation: Option<PublicListingSourceSearchContinuation>,
}

impl SearchPublicListingSourcesRequest {
    pub fn new(
        query: PublicListingSourceSearchQuery,
        page_size: u8,
        continuation: Option<PublicListingSourceSearchContinuation>,
    ) -> Result<Self, SearchPublicListingSourcesRequestError> {
        if !(1..=MAX_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE).contains(&page_size) {
            return Err(SearchPublicListingSourcesRequestError::PageSizeOutOfRange {
                page_size,
                max: MAX_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE,
            });
        }

        if query.is_insufficient_input() && continuation.is_some() {
            return Err(SearchPublicListingSourcesRequestError::ContinuationForInsufficientInput);
        }

        if let Some(continuation) = &continuation {
            if query.is_browse() && continuation.position.match_tier != 0 {
                return Err(
                    SearchPublicListingSourcesRequestError::InvalidBrowseMatchTier {
                        match_tier: continuation.position.match_tier,
                    },
                );
            }
            if continuation.binding != query_binding(&query, page_size) {
                return Err(SearchPublicListingSourcesRequestError::ContinuationBindingMismatch);
            }
        }

        Ok(Self {
            query,
            page_size,
            continuation,
        })
    }

    pub fn query(&self) -> &PublicListingSourceSearchQuery {
        &self.query
    }

    pub const fn page_size(&self) -> u8 {
        self.page_size
    }

    pub fn continuation(&self) -> Option<&PublicListingSourceSearchContinuation> {
        self.continuation.as_ref()
    }

    fn binding(&self) -> [u8; 32] {
        query_binding(&self.query, self.page_size)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchPublicListingSourcesRequestError {
    #[error("public listing source search page size {page_size} is outside 1 through {max}")]
    PageSizeOutOfRange { page_size: u8, max: u8 },
    #[error("public listing source search continuation is invalid for insufficient input")]
    ContinuationForInsufficientInput,
    #[error("public listing source browse continuation must have match tier zero")]
    InvalidBrowseMatchTier { match_tier: u8 },
    #[error("public listing source search continuation does not match the query and page size")]
    ContinuationBindingMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicListingSourceSearchPage {
    pub items: Vec<PublicListingSourceSummary>,
    pub next_position: Option<PublicListingSourceSearchPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPublicListingSourcesResult {
    pub items: Vec<PublicListingSourceSummary>,
    pub page_size: u8,
    pub continuation: Option<PublicListingSourceSearchContinuation>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchPublicListingSourcesError {
    #[error("temporary public listing source search failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid public listing source search read model")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("internal public listing source search failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin public listing source search transaction")]
    BeginTransaction {
        #[source]
        source: TransactionError,
    },
    #[error("failed to commit public listing source search transaction")]
    CommitTransaction {
        #[source]
        source: TransactionError,
    },
}

#[async_trait::async_trait]
pub trait SearchPublicListingSourcesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchPublicListingSourcesRequest,
    ) -> Result<SearchPublicListingSourcesResult, SearchPublicListingSourcesError>;
}

pub struct SearchPublicListingSourcesHandler<U, R> {
    unit_of_work: U,
    reader: R,
}

impl<U, R> SearchPublicListingSourcesHandler<U, R> {
    pub fn new(unit_of_work: U, reader: R) -> Self {
        Self {
            unit_of_work,
            reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, R> SearchPublicListingSourcesUseCase for SearchPublicListingSourcesHandler<U, R>
where
    U: UnitOfWork,
    R: PublicListingSourceSearchReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "search_public_listing_sources",
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
        request: SearchPublicListingSourcesRequest,
    ) -> Result<SearchPublicListingSourcesResult, SearchPublicListingSourcesError> {
        if request.query.is_insufficient_input() {
            return Ok(SearchPublicListingSourcesResult {
                items: vec![],
                page_size: request.page_size,
                continuation: None,
            });
        }

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|source| SearchPublicListingSourcesError::BeginTransaction { source })?;
        let page = self.reader.in_transaction(&mut tx).search(&request).await?;
        tx.commit()
            .await
            .map_err(|source| SearchPublicListingSourcesError::CommitTransaction { source })?;

        Ok(SearchPublicListingSourcesResult {
            items: page.items,
            page_size: request.page_size,
            continuation: page.next_position.map(|position| {
                PublicListingSourceSearchContinuation::new(request.binding(), position)
            }),
        })
    }
}

impl From<PublicListingSourceSearchReadError> for SearchPublicListingSourcesError {
    fn from(value: PublicListingSourceSearchReadError) -> Self {
        match value {
            PublicListingSourceSearchReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PublicListingSourceSearchReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
            PublicListingSourceSearchReadError::Internal { source } => Self::Internal { source },
        }
    }
}

fn canonicalize_query(value: &str) -> String {
    let mut canonical = String::new();
    let mut previous_was_whitespace = false;

    for character in value.nfc() {
        if is_unicode_white_space(character) {
            if !canonical.is_empty() && !previous_was_whitespace {
                canonical.push(' ');
            }
            previous_was_whitespace = true;
        } else {
            canonical.push(character);
            previous_was_whitespace = false;
        }
    }

    if previous_was_whitespace {
        canonical.pop();
    }
    canonical
}

fn validate_query_limits(value: &str) -> Result<(), PublicListingSourceSearchQueryError> {
    if value.len() > MAX_PUBLIC_LISTING_SOURCE_SEARCH_QUERY_UTF8_BYTES {
        return Err(PublicListingSourceSearchQueryError::TooManyUtf8Bytes {
            max_bytes: MAX_PUBLIC_LISTING_SOURCE_SEARCH_QUERY_UTF8_BYTES,
        });
    }
    if value.chars().count() > MAX_PUBLIC_LISTING_SOURCE_SEARCH_QUERY_SCALARS {
        return Err(PublicListingSourceSearchQueryError::TooManyUnicodeScalars {
            max_scalars: MAX_PUBLIC_LISTING_SOURCE_SEARCH_QUERY_SCALARS,
        });
    }
    Ok(())
}

fn reject_non_whitespace_controls(value: &str) -> Result<(), PublicListingSourceSearchQueryError> {
    if value
        .chars()
        .any(|character| character.is_control() && !is_unicode_white_space(character))
    {
        Err(PublicListingSourceSearchQueryError::NonWhitespaceControlCharacter)
    } else {
        Ok(())
    }
}

const fn is_unicode_white_space(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{0085}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}

fn query_binding(query: &PublicListingSourceSearchQuery, page_size: u8) -> [u8; 32] {
    let mut hasher = Sha256::new();
    write_binding_part(&mut hasher, ENDPOINT_IDENTITY);
    write_binding_part(&mut hasher, query.mode_identity());
    write_binding_part(
        &mut hasher,
        query.canonical_text().unwrap_or_default().as_bytes(),
    );
    write_binding_part(&mut hasher, SORT_IDENTITY);
    write_binding_part(&mut hasher, &[page_size]);
    hasher.finalize().into()
}

fn write_binding_part(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ports::{PublicListingSourceSearchReader, PublicListingSourceSearchReaderFactory},
        use_cases::queries::public_listing_source::{
            PublicListingSourceOperatorSummary, PublicListingSourceSummary,
        },
    };
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
    };
    use listing_source_core::{ListingSourceName, ListingSourceSlugId};
    use party_core::party_name::PartyName;
    use std::sync::{Arc, Mutex, MutexGuard};

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
    enum ReaderFailure {
        TemporarilyUnavailable,
        InvalidReadModel,
        Internal,
    }

    #[derive(Clone)]
    struct FakeReaderFactory {
        state: Arc<Mutex<State>>,
        page: PublicListingSourceSearchPage,
        failure: Option<ReaderFailure>,
    }

    struct FakeReader {
        state: Arc<Mutex<State>>,
        page: PublicListingSourceSearchPage,
        failure: Option<ReaderFailure>,
    }

    impl PublicListingSourceSearchReaderFactory<FakeTransaction> for FakeReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl PublicListingSourceSearchReader + 'tx {
            lock(&self.state).bindings += 1;
            FakeReader {
                state: Arc::clone(&self.state),
                page: self.page.clone(),
                failure: self.failure,
            }
        }
    }

    #[async_trait::async_trait]
    impl PublicListingSourceSearchReader for FakeReader {
        async fn search(
            &mut self,
            _request: &SearchPublicListingSourcesRequest,
        ) -> Result<PublicListingSourceSearchPage, PublicListingSourceSearchReadError> {
            lock(&self.state).reads += 1;
            match self.failure {
                None => Ok(self.page.clone()),
                Some(ReaderFailure::TemporarilyUnavailable) => {
                    Err(PublicListingSourceSearchReadError::TemporarilyUnavailable {
                        source: static_error("reader unavailable"),
                    })
                }
                Some(ReaderFailure::InvalidReadModel) => {
                    Err(PublicListingSourceSearchReadError::InvalidReadModel {
                        source: static_error("invalid row"),
                    })
                }
                Some(ReaderFailure::Internal) => {
                    Err(PublicListingSourceSearchReadError::Internal {
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

    fn query(
        value: Option<&str>,
    ) -> Result<PublicListingSourceSearchQuery, Box<dyn std::error::Error>> {
        PublicListingSourceSearchQuery::new(value.map(str::to_owned)).map_err(Into::into)
    }

    fn request(
        value: Option<&str>,
    ) -> Result<SearchPublicListingSourcesRequest, Box<dyn std::error::Error>> {
        SearchPublicListingSourcesRequest::new(
            query(value)?,
            DEFAULT_PUBLIC_LISTING_SOURCE_SEARCH_PAGE_SIZE,
            None,
        )
        .map_err(Into::into)
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
        failure: Option<ReaderFailure>,
        begin_fails: bool,
        commit_fails: bool,
    ) -> Result<
        SearchPublicListingSourcesHandler<FakeUnitOfWork, FakeReaderFactory>,
        Box<dyn std::error::Error>,
    > {
        Ok(SearchPublicListingSourcesHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(&state),
                begin_fails,
                commit_fails,
            },
            FakeReaderFactory {
                state,
                page: PublicListingSourceSearchPage {
                    items: vec![summary()?],
                    next_position: Some(PublicListingSourceSearchPosition::new(
                        0,
                        "source".to_owned(),
                        ListingSourceId::new(),
                    )?),
                },
                failure,
            },
        ))
    }

    #[test]
    fn should_canonicalize_nfc_and_unicode_whitespace() -> Result<(), Box<dyn std::error::Error>> {
        let value = PublicListingSourceSearchQuery::new(Some(
            "\u{00A0}Müller\u{202F}\u{3000}Auktionshaus\u{00A0}".to_owned(),
        ))?;

        assert_eq!(Some("Müller Auktionshaus"), value.canonical_text());
        Ok(())
    }

    #[test]
    fn should_classify_browse_and_insufficient_inputs() -> Result<(), Box<dyn std::error::Error>> {
        assert!(PublicListingSourceSearchQuery::new(None)?.is_browse());
        assert!(PublicListingSourceSearchQuery::new(Some(" \t\n ".to_owned()))?.is_browse());
        assert!(PublicListingSourceSearchQuery::new(Some("m".to_owned()))?.is_insufficient_input());
        assert!(
            PublicListingSourceSearchQuery::new(Some("%_".to_owned()))?.is_insufficient_input()
        );
        assert_eq!(
            Some("mu"),
            PublicListingSourceSearchQuery::new(Some("mu".to_owned()))?.canonical_text()
        );
        Ok(())
    }

    #[test]
    fn should_reject_query_limits_and_non_whitespace_controls() {
        assert!(matches!(
            PublicListingSourceSearchQuery::new(Some("é".repeat(513))),
            Err(PublicListingSourceSearchQueryError::TooManyUtf8Bytes { .. })
        ));
        assert!(matches!(
            PublicListingSourceSearchQuery::new(Some("a".repeat(257))),
            Err(PublicListingSourceSearchQueryError::TooManyUnicodeScalars { .. })
        ));
        assert!(matches!(
            PublicListingSourceSearchQuery::new(Some("ab\0cd".to_owned())),
            Err(PublicListingSourceSearchQueryError::NonWhitespaceControlCharacter)
        ));
    }

    #[test]
    fn should_bind_continuations_to_the_canonical_query_and_page_size()
    -> Result<(), Box<dyn std::error::Error>> {
        let position =
            PublicListingSourceSearchPosition::new(0, "source".to_owned(), ListingSourceId::new())?;
        let initial = SearchPublicListingSourcesRequest::new(query(Some("  Müller\t"))?, 21, None)?;
        let continuation =
            PublicListingSourceSearchContinuation::new(initial.binding(), position.clone());

        assert!(
            SearchPublicListingSourcesRequest::new(
                query(Some("Müller"))?,
                21,
                Some(continuation.clone())
            )
            .is_ok()
        );
        assert!(matches!(
            SearchPublicListingSourcesRequest::new(query(Some("Müller"))?, 22, Some(continuation)),
            Err(SearchPublicListingSourcesRequestError::ContinuationBindingMismatch)
        ));

        let browse = PublicListingSourceSearchQuery::new(None)?;
        let continuation = PublicListingSourceSearchContinuation::new([9; 32], position.clone());
        assert!(matches!(
            SearchPublicListingSourcesRequest::new(browse, 21, Some(continuation)),
            Err(SearchPublicListingSourcesRequestError::ContinuationBindingMismatch)
        ));
        let insufficient = PublicListingSourceSearchQuery::new(Some("m".to_owned()))?;
        let continuation = PublicListingSourceSearchContinuation::new([0; 32], position);
        assert!(matches!(
            SearchPublicListingSourcesRequest::new(insufficient, 21, Some(continuation)),
            Err(SearchPublicListingSourcesRequestError::ContinuationForInsufficientInput)
        ));
        Ok(())
    }

    #[test]
    fn should_reject_out_of_range_page_sizes_and_positions()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(matches!(
            SearchPublicListingSourcesRequest::new(query(Some("mu"))?, 0, None),
            Err(SearchPublicListingSourcesRequestError::PageSizeOutOfRange { .. })
        ));
        assert!(matches!(
            PublicListingSourceSearchPosition::new(6, "source".to_owned(), ListingSourceId::new()),
            Err(PublicListingSourceSearchPositionError::InvalidMatchTier { .. })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_allow_anonymous_search_and_preserve_reader_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = handler(Arc::clone(&state), None, false, false)?;

        let result = handler
            .execute(&context(Principal::Anonymous), request(Some("mu"))?)
            .await?;

        assert_eq!(1, result.items.len());
        assert!(result.continuation.is_some());
        let state = lock(&state);
        assert_eq!(1, state.begins);
        assert_eq!(1, state.bindings);
        assert_eq!(1, state.reads);
        assert_eq!(1, state.commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_insufficient_input_without_starting_transaction()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let handler = handler(Arc::clone(&state), None, false, false)?;

        let result = handler
            .execute(&context(Principal::Anonymous), request(Some("m"))?)
            .await?;

        assert!(result.items.is_empty());
        assert!(result.continuation.is_none());
        let state = lock(&state);
        assert_eq!(0, state.begins);
        assert_eq!(0, state.bindings);
        assert_eq!(0, state.reads);
        assert_eq!(0, state.commits);
        Ok(())
    }

    #[tokio::test]
    async fn should_preserve_reader_errors_without_committing()
    -> Result<(), Box<dyn std::error::Error>> {
        for failure in [
            ReaderFailure::TemporarilyUnavailable,
            ReaderFailure::InvalidReadModel,
            ReaderFailure::Internal,
        ] {
            let state = Arc::new(Mutex::new(State::default()));
            let handler = handler(Arc::clone(&state), Some(failure), false, false)?;

            let result = handler
                .execute(&context(Principal::Anonymous), request(Some("mu"))?)
                .await;

            assert!(matches!(
                result,
                Err(SearchPublicListingSourcesError::TemporarilyUnavailable { .. })
                    | Err(SearchPublicListingSourcesError::InvalidReadModel { .. })
                    | Err(SearchPublicListingSourcesError::Internal { .. })
            ));
            assert_eq!(0, lock(&state).commits);
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_map_transaction_failures() -> Result<(), Box<dyn std::error::Error>> {
        let state = Arc::new(Mutex::new(State::default()));
        let begin_failure = handler(Arc::clone(&state), None, true, false)?
            .execute(&context(Principal::Anonymous), request(Some("mu"))?)
            .await;
        assert!(matches!(
            begin_failure,
            Err(SearchPublicListingSourcesError::BeginTransaction { .. })
        ));
        assert_eq!(0, lock(&state).reads);

        let state = Arc::new(Mutex::new(State::default()));
        let commit_failure = handler(Arc::clone(&state), None, false, true)?
            .execute(&context(Principal::Anonymous), request(Some("mu"))?)
            .await;
        assert!(matches!(
            commit_failure,
            Err(SearchPublicListingSourcesError::CommitTransaction { .. })
        ));
        assert_eq!(1, lock(&state).reads);
        assert_eq!(0, lock(&state).commits);
        Ok(())
    }
}

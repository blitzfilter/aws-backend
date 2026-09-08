use application::error::BoxError;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::PriceMatchValuation;
use search_filter_core::enhanced_match_reason::EnhancedMatchReason;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilterMatchCandidate {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub expected_search: ProductListingSearch,
    pub expected_embedding: Option<Vec<f32>>,
    pub price_match_valuation: Option<PriceMatchValuation>,
    pub enhanced_match_reason: Option<EnhancedMatchReason>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSearchFilterMatchCandidate {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub search_filter_name: UserSearchFilterName,
    pub price_match_valuation: Option<PriceMatchValuation>,
    pub enhanced_match_reason: Option<EnhancedMatchReason>,
}

#[derive(Debug, thiserror::Error)]
pub enum ActiveSearchFilterMatchCandidateReadError {
    #[error("active search filter match candidate read failed")]
    ReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("active search filter match candidate state is invalid")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait ActiveSearchFilterMatchCandidateReader: Send {
    /// Lock active filters through match commit and retain only the exact evaluated search inputs.
    /// Index views have no storage revision; unrelated name/notification changes remain eligible.
    async fn find_active(
        &mut self,
        candidates: &[SearchFilterMatchCandidate],
    ) -> Result<Vec<ActiveSearchFilterMatchCandidate>, ActiveSearchFilterMatchCandidateReadError>;
}

pub trait ActiveSearchFilterMatchCandidateReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut Tx,
    ) -> impl ActiveSearchFilterMatchCandidateReader + 'tx;
}

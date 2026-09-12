use crate::ports::{
    SearchFilterQuotaReadError, SearchFilterQuotaReader, SearchFilterQuotaReaderFactory,
    SearchFilterReadError, SearchFilterReader, SearchFilterRepository, SearchFilterRepositoryError,
    SearchFilterRepositoryFactory, SearchFilterView,
};
use crate::tier_policy::{
    active_filter_quota, validate_search_feature_changes, validate_search_features,
};
use crate::use_cases::embedding_query;
use application::error::{BoxError, box_error};
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext,
};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::query::any_of_query::AnyOfQuery;
use domain_primitives::query::range_query::RangeQuery;
use domain_primitives::query::text_query::TextQuery;
use embedding::{EmbeddingError, EmbeddingGenerator};
use listing_source_core::ListingSourceId;
use localization::Language;
use money::{Currency, MonetaryAmount};
use product_listing_core::product_listing_search::{
    EnhancedSearchDescription, ListingAvailabilityQuery, ProductListingSearch,
};
use search_filter_core::search_filter_state::SearchFilterState;
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use search_filter_core::user_search_filter_name::UserSearchFilterName;
use time::OffsetDateTime;
use user_core::user_id::UserId;
use user_service::ports::{
    UserTierEntitlements, UserTierEntitlementsError, UserTierEntitlementsFactory,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProductListingSearchPatch {
    pub language: PatchField<Language>,
    pub currency: PatchField<Currency>,
    pub product_listing_query: PatchField<Vec<TextQuery<1>>>,
    pub enhanced_search_description: PatchField<EnhancedSearchDescription>,
    pub listing_source_id_query: PatchField<AnyOfQuery<ListingSourceId>>,
    pub exclude_listing_source_id_query: PatchField<AnyOfQuery<ListingSourceId>>,
    pub price_query: PatchField<RangeQuery<MonetaryAmount>>,
    pub availability_query: PatchField<ListingAvailabilityQuery>,
    pub created_query: PatchField<RangeQuery<OffsetDateTime>>,
    pub updated_query: PatchField<RangeQuery<OffsetDateTime>>,
    pub lot_bidding_opens_query: PatchField<RangeQuery<OffsetDateTime>>,
    pub lot_scheduled_closes_query: PatchField<RangeQuery<OffsetDateTime>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateOwnedSearchFilterCommand {
    pub user_id: UserId,
    pub search_filter_id: UserSearchFilterId,
    pub name: PatchField<UserSearchFilterName>,
    pub notifications: PatchField<bool>,
    pub state: PatchField<SearchFilterState>,
    pub search: ProductListingSearchPatch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateOwnedSearchFilterResult {
    pub filter: SearchFilterView,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateOwnedSearchFilterError {
    #[error("authenticated actor required")]
    AuthenticatedActorRequired,
    #[error("actor may not manage this search filter")]
    ActorMayNotManageSearchFilter,
    #[error("user not found")]
    UserNotFound,
    #[error(
        "search filter quota exceeded: {active_count}/{quota} active filters are already in use"
    )]
    SearchFilterQuotaExceeded { active_count: usize, quota: usize },
    #[error("search filter feature '{feature}' requires a higher user tier")]
    SearchFilterFeatureRestricted { feature: &'static str },
    #[error("user tier entitlement lock failed")]
    UserTierEntitlementsLockFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter quota read failed")]
    SearchFilterQuotaReadFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter not found")]
    SearchFilterNotFound,
    #[error("search filter patch cannot clear required fields")]
    InvalidSearchFilterPatch,
    #[error("search filter embedding generation failed")]
    EmbeddingGenerationFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter lookup failed")]
    SearchFilterLookupFailed {
        #[source]
        source: BoxError,
    },
    #[error("search filter update conflicted with a concurrent write")]
    SearchFilterConcurrencyConflict,
    #[error("search filter update failed")]
    SearchFilterUpdateFailed {
        #[source]
        source: BoxError,
    },
    #[error("persisted search filter state is invalid")]
    PersistedSearchFilterStateInvalid {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin search filter transaction")]
    BeginTransactionFailed,
    #[error("failed to commit search filter transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait UpdateOwnedSearchFilterUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateOwnedSearchFilterCommand,
    ) -> Result<UpdateOwnedSearchFilterResult, UpdateOwnedSearchFilterError>;
}

pub struct UpdateOwnedSearchFilterHandler<U, R, E, F, Q, A> {
    unit_of_work: U,
    filters: R,
    embeddings: E,
    filter_reader: F,
    quotas: Q,
    tier_entitlements: A,
}

impl<U, R, E, F, Q, A> UpdateOwnedSearchFilterHandler<U, R, E, F, Q, A> {
    pub fn new(
        unit_of_work: U,
        filters: R,
        embeddings: E,
        filter_reader: F,
        quotas: Q,
        tier_entitlements: A,
    ) -> Self {
        Self {
            unit_of_work,
            filters,
            embeddings,
            filter_reader,
            quotas,
            tier_entitlements,
        }
    }
}

#[async_trait::async_trait]
impl<U, R, E, F, Q, A> UpdateOwnedSearchFilterUseCase
    for UpdateOwnedSearchFilterHandler<U, R, E, F, Q, A>
where
    U: UnitOfWork,
    R: SearchFilterRepositoryFactory<U::Tx>,
    E: EmbeddingGenerator,
    F: SearchFilterReader,
    Q: SearchFilterQuotaReaderFactory<U::Tx>,
    A: UserTierEntitlementsFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "update_owned_search_filter",
        skip_all,
        fields(
            search_filter_id = %command.search_filter_id,
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdateOwnedSearchFilterCommand,
    ) -> Result<UpdateOwnedSearchFilterResult, UpdateOwnedSearchFilterError> {
        authorize_owner(context, command.user_id)?;
        let prepared_view = self
            .filter_reader
            .find_for_user_by_id(command.user_id, command.search_filter_id)
            .await
            .map_err(preparation_read_error)?
            .ok_or(UpdateOwnedSearchFilterError::SearchFilterNotFound)?;
        let mut prepared_search = prepared_view.search;
        let prepared_search_changed =
            apply_product_search_patch(&mut prepared_search, &command.search)?;
        let prepared_embedding = if prepared_search_changed {
            Some(
                match embedding_query(&prepared_search).map_err(embedding_error)? {
                    Some(query) => Some(
                        self.embeddings
                            .embed_search_query(&query)
                            .await
                            .map_err(embedding_error)?
                            .into_values(),
                    ),
                    None => None,
                },
            )
        } else {
            None
        };

        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| UpdateOwnedSearchFilterError::BeginTransactionFailed)?;
        let tier = self
            .tier_entitlements
            .in_transaction(&mut tx)
            .lock_user_tier(command.user_id)
            .await
            .map_err(tier_entitlements_error)?
            .ok_or(UpdateOwnedSearchFilterError::UserNotFound)?;
        let mut persisted = self
            .filters
            .in_transaction(&mut tx)
            .find_by_id(command.search_filter_id)
            .await
            .map_err(lookup_error)?
            .filter(|persisted| persisted.filter.user_id() == command.user_id)
            .ok_or(UpdateOwnedSearchFilterError::SearchFilterNotFound)?;
        let mut filter = persisted.filter.clone();

        let changed = apply_filter_patch(&mut filter, &command)?;
        validate_search_feature_changes(tier, persisted.filter.search(), filter.search()).map_err(
            |feature| UpdateOwnedSearchFilterError::SearchFilterFeatureRestricted { feature },
        )?;
        let reactivating = !persisted.filter.state().is_active() && filter.state().is_active();
        if reactivating {
            validate_search_features(tier, filter.search()).map_err(|feature| {
                UpdateOwnedSearchFilterError::SearchFilterFeatureRestricted { feature }
            })?;
            let quota = active_filter_quota(tier);
            let active_count = self
                .quotas
                .in_transaction(&mut tx)
                .count_active_for_user(command.user_id)
                .await
                .map_err(search_filter_quota_read_error)?;
            if active_count >= quota {
                return Err(UpdateOwnedSearchFilterError::SearchFilterQuotaExceeded {
                    active_count,
                    quota,
                });
            }
        }
        if changed.search_changed {
            if !prepared_search_changed || filter.search() != &prepared_search {
                return Err(UpdateOwnedSearchFilterError::SearchFilterConcurrencyConflict);
            }
            let embedding = prepared_embedding
                .ok_or(UpdateOwnedSearchFilterError::SearchFilterConcurrencyConflict)?;
            let _ = filter.replace_search(filter.search().clone(), embedding);
        }
        if changed.aggregate_changed {
            persisted = self
                .filters
                .in_transaction(&mut tx)
                .update(&filter, persisted.version)
                .await
                .map_err(update_error)?;
        }
        tx.commit()
            .await
            .map_err(|_| UpdateOwnedSearchFilterError::CommitTransactionFailed)?;
        tracing::info!(
            event = "search_filter.updated",
            actor_type = context.principal.kind(),
            actor_id = ?context.principal.actor_id(),
            search_filter_id = %persisted.filter.id(),
            outcome = if changed.aggregate_changed { "success" } else { "unchanged" },
        );

        Ok(UpdateOwnedSearchFilterResult {
            filter: persisted.into(),
        })
    }
}

#[derive(Default)]
struct PatchOutcome {
    aggregate_changed: bool,
    search_changed: bool,
}

fn apply_filter_patch(
    filter: &mut search_filter_core::SearchFilter,
    command: &UpdateOwnedSearchFilterCommand,
) -> Result<PatchOutcome, UpdateOwnedSearchFilterError> {
    let mut outcome = PatchOutcome::default();
    match &command.name {
        PatchField::Unchanged => {}
        PatchField::Set(name) => outcome.aggregate_changed |= filter.rename(name.clone()).changed(),
        PatchField::Clear => return Err(UpdateOwnedSearchFilterError::InvalidSearchFilterPatch),
    }
    match &command.notifications {
        PatchField::Unchanged => {}
        PatchField::Set(notifications) => {
            outcome.aggregate_changed |= filter.change_notifications(*notifications).changed();
        }
        PatchField::Clear => return Err(UpdateOwnedSearchFilterError::InvalidSearchFilterPatch),
    }
    match &command.state {
        PatchField::Unchanged => {}
        PatchField::Set(state) => {
            outcome.aggregate_changed |= filter.change_state(*state).changed();
        }
        PatchField::Clear => return Err(UpdateOwnedSearchFilterError::InvalidSearchFilterPatch),
    }

    let mut search = filter.search().clone();
    outcome.search_changed = apply_product_search_patch(&mut search, &command.search)?;
    if outcome.search_changed {
        outcome.aggregate_changed |= filter
            .replace_search(search, filter.embedding().cloned())
            .changed();
    }
    Ok(outcome)
}

fn apply_product_search_patch(
    search: &mut ProductListingSearch,
    patch: &ProductListingSearchPatch,
) -> Result<bool, UpdateOwnedSearchFilterError> {
    let mut changed = false;
    changed |= apply_required_patch(&patch.language, &mut search.language)?;
    changed |= apply_required_patch(&patch.currency, &mut search.currency)?;
    changed |= apply_default_patch(
        &patch.product_listing_query,
        &mut search.product_listing_query,
    );
    changed |= apply_optional_patch(
        &patch.enhanced_search_description,
        &mut search.enhanced_search_description,
    );
    changed |= apply_default_patch(
        &patch.listing_source_id_query,
        &mut search.listing_source_id_query,
    );
    changed |= apply_default_patch(
        &patch.exclude_listing_source_id_query,
        &mut search.exclude_listing_source_id_query,
    );
    changed |= apply_optional_patch(&patch.price_query, &mut search.price_query);
    changed |= apply_optional_patch(&patch.availability_query, &mut search.availability_query);
    changed |= apply_optional_patch(&patch.created_query, &mut search.created_query);
    changed |= apply_optional_patch(&patch.updated_query, &mut search.updated_query);
    changed |= apply_optional_patch(
        &patch.lot_bidding_opens_query,
        &mut search.lot_bidding_opens_query,
    );
    changed |= apply_optional_patch(
        &patch.lot_scheduled_closes_query,
        &mut search.lot_scheduled_closes_query,
    );
    Ok(changed)
}

fn apply_required_patch<T: Clone + PartialEq>(
    patch: &PatchField<T>,
    target: &mut T,
) -> Result<bool, UpdateOwnedSearchFilterError> {
    match patch {
        PatchField::Unchanged => Ok(false),
        PatchField::Set(value) => Ok(replace_if_changed(target, value.clone())),
        PatchField::Clear => Err(UpdateOwnedSearchFilterError::InvalidSearchFilterPatch),
    }
}

fn apply_default_patch<T: Clone + Default + PartialEq>(
    patch: &PatchField<T>,
    target: &mut T,
) -> bool {
    match patch {
        PatchField::Unchanged => false,
        PatchField::Set(value) => replace_if_changed(target, value.clone()),
        PatchField::Clear => replace_if_changed(target, T::default()),
    }
}

fn apply_optional_patch<T: Clone + PartialEq>(
    patch: &PatchField<T>,
    target: &mut Option<T>,
) -> bool {
    match patch {
        PatchField::Unchanged => false,
        PatchField::Set(value) => replace_if_changed(target, Some(value.clone())),
        PatchField::Clear => replace_if_changed(target, None),
    }
}

fn replace_if_changed<T: PartialEq>(target: &mut T, replacement: T) -> bool {
    if *target == replacement {
        false
    } else {
        *target = replacement;
        true
    }
}

fn authorize_owner(
    context: &OperationContext,
    user_id: UserId,
) -> Result<(), UpdateOwnedSearchFilterError> {
    context
        .require()
        .credential_capability(CredentialCapability::SearchFiltersWrite)
        .user(&user_id)
        .service_or_system()
        .authorize::<UpdateOwnedSearchFilterError>()
}

fn embedding_error(error: EmbeddingError) -> UpdateOwnedSearchFilterError {
    UpdateOwnedSearchFilterError::EmbeddingGenerationFailed {
        source: box_error(error),
    }
}

fn tier_entitlements_error(error: UserTierEntitlementsError) -> UpdateOwnedSearchFilterError {
    match error {
        UserTierEntitlementsError::LockFailed { source }
        | UserTierEntitlementsError::ReconciliationFailed { source } => {
            UpdateOwnedSearchFilterError::UserTierEntitlementsLockFailed { source }
        }
    }
}

fn preparation_read_error(error: SearchFilterReadError) -> UpdateOwnedSearchFilterError {
    UpdateOwnedSearchFilterError::SearchFilterLookupFailed {
        source: box_error(error),
    }
}

fn search_filter_quota_read_error(
    error: SearchFilterQuotaReadError,
) -> UpdateOwnedSearchFilterError {
    UpdateOwnedSearchFilterError::SearchFilterQuotaReadFailed {
        source: box_error(error),
    }
}

fn lookup_error(error: SearchFilterRepositoryError) -> UpdateOwnedSearchFilterError {
    match error {
        SearchFilterRepositoryError::InvalidPersistedState { source } => {
            UpdateOwnedSearchFilterError::PersistedSearchFilterStateInvalid { source }
        }
        error => UpdateOwnedSearchFilterError::SearchFilterLookupFailed {
            source: box_error(error),
        },
    }
}

fn update_error(error: SearchFilterRepositoryError) -> UpdateOwnedSearchFilterError {
    match error {
        SearchFilterRepositoryError::InvalidPersistedState { source } => {
            UpdateOwnedSearchFilterError::PersistedSearchFilterStateInvalid { source }
        }
        SearchFilterRepositoryError::ConcurrencyConflict => {
            UpdateOwnedSearchFilterError::SearchFilterConcurrencyConflict
        }
        error => UpdateOwnedSearchFilterError::SearchFilterUpdateFailed {
            source: box_error(error),
        },
    }
}

impl From<OperationAuthorizationError> for UpdateOwnedSearchFilterError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => {
                Self::ActorMayNotManageSearchFilter
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_merge_only_patched_search_fields() -> Result<(), Box<dyn std::error::Error>> {
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        search.enhanced_search_description =
            Some(EnhancedSearchDescription::try_from("gold ring")?);
        let patch = ProductListingSearchPatch {
            language: PatchField::Set(Language::De),
            ..Default::default()
        };

        let changed = apply_product_search_patch(&mut search, &patch)?;

        assert!(changed);
        assert_eq!(Language::De, search.language);
        assert_eq!(Currency::Eur, search.currency);
        assert_eq!(
            Some(EnhancedSearchDescription::try_from("gold ring")?),
            search.enhanced_search_description
        );
        Ok(())
    }

    #[test]
    fn should_replace_and_clear_the_optional_availability_query()
    -> Result<(), Box<dyn std::error::Error>> {
        use product_listing_core::{
            listing_availability::ListingAvailability, listing_orderability::ListingOrderability,
        };
        use std::collections::HashSet;

        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        let query = ListingAvailabilityQuery {
            any_of: HashSet::from([ListingAvailability::InStock]).into(),
            orderability: HashSet::from([ListingOrderability::OrderableNow]).into(),
            include_unspecified: true,
        };
        let patch = ProductListingSearchPatch {
            availability_query: PatchField::Set(query.clone()),
            ..Default::default()
        };

        assert!(apply_product_search_patch(&mut search, &patch)?);
        assert_eq!(Some(query), search.availability_query);

        let clear = ProductListingSearchPatch {
            availability_query: PatchField::Clear,
            ..Default::default()
        };
        assert!(apply_product_search_patch(&mut search, &clear)?);
        assert_eq!(None, search.availability_query);
        Ok(())
    }

    #[test]
    fn should_apply_and_clear_listing_source_id_queries() -> Result<(), UpdateOwnedSearchFilterError>
    {
        use std::collections::HashSet;

        let listing_source_id = ListingSourceId::new();
        let excluded_listing_source_id = ListingSourceId::new();
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        let patch = ProductListingSearchPatch {
            listing_source_id_query: PatchField::Set(HashSet::from([listing_source_id]).into()),
            exclude_listing_source_id_query: PatchField::Set(
                HashSet::from([excluded_listing_source_id]).into(),
            ),
            ..Default::default()
        };

        assert!(apply_product_search_patch(&mut search, &patch)?);
        assert!(search.listing_source_id_query.contains(&listing_source_id));
        assert!(
            search
                .exclude_listing_source_id_query
                .contains(&excluded_listing_source_id)
        );

        let clear = ProductListingSearchPatch {
            listing_source_id_query: PatchField::Clear,
            exclude_listing_source_id_query: PatchField::Clear,
            ..Default::default()
        };

        assert!(apply_product_search_patch(&mut search, &clear)?);
        assert!(search.listing_source_id_query.is_empty());
        assert!(search.exclude_listing_source_id_query.is_empty());
        Ok(())
    }

    #[test]
    fn should_clear_optional_enhanced_search_description() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        search.enhanced_search_description =
            Some(EnhancedSearchDescription::try_from("gold ring")?);
        let patch = ProductListingSearchPatch {
            enhanced_search_description: PatchField::Clear,
            ..Default::default()
        };

        let changed = apply_product_search_patch(&mut search, &patch)?;

        assert!(changed);
        assert_eq!(None, search.enhanced_search_description);
        Ok(())
    }

    #[test]
    fn should_not_change_search_when_patch_is_empty() -> Result<(), UpdateOwnedSearchFilterError> {
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);

        let changed =
            apply_product_search_patch(&mut search, &ProductListingSearchPatch::default())?;

        assert!(!changed);
        Ok(())
    }

    #[test]
    fn should_not_mark_search_changed_when_patch_repeats_existing_value()
    -> Result<(), UpdateOwnedSearchFilterError> {
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        let patch = ProductListingSearchPatch {
            language: PatchField::Set(Language::En),
            ..Default::default()
        };

        let changed = apply_product_search_patch(&mut search, &patch)?;

        assert!(!changed);
        Ok(())
    }

    #[test]
    fn should_reject_clearing_required_search_language() {
        let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
        let patch = ProductListingSearchPatch {
            language: PatchField::Clear,
            ..Default::default()
        };

        let result = apply_product_search_patch(&mut search, &patch);

        assert!(matches!(
            result,
            Err(UpdateOwnedSearchFilterError::InvalidSearchFilterPatch)
        ));
    }
}

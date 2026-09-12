use crate::ports::{
    ListingSourceSummaryReadError, ListingSourceSummaryReader, ListingSourceSummaryWithReferral,
    ProductListingUserStateLookup, ProductListingUserStateReadError, ProductListingUserStateReader,
};
use crate::use_cases::queries::search_product_listings::{
    PersonalizedProductListingSearchItem, ProductListingSearchItem,
    ProductListingSearchItemWithSource,
};
use application::error::{BoxError, box_error};
use domain_primitives::event_id::EventId;
use localization::{Language, Localized};
use product_listing_core::listing_lifecycle::ListingLifecycle;
use product_listing_core::product_listing_id::ProductListingId;
#[cfg(test)]
use product_listing_core::product_listing_slug_id::ProductListingSlugId;

use crate::ports::ListingSourceSummary;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId, outbound_url};
use product_listing_core::source_listing_id::SourceListingId;
use user_core::user_id::UserId;

use product_listing_core::title::Title;

use indexmap::IndexSet;
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProductListingSummaryPersonalizationError {
    #[error("listing source summary query failed")]
    ListingSourceSummaryQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("listing source summary read model is invalid")]
    ListingSourceSummaryReadModelInvalid {
        #[source]
        source: BoxError,
    },
    #[error("listing source summary is missing for listing source {listing_source_id}")]
    ListingSourceSummaryMissing { listing_source_id: ListingSourceId },
    #[error("product user state query failed")]
    UserStateQueryFailed {
        #[source]
        source: BoxError,
    },
    #[error("product user state read model is invalid")]
    UserStateReadModelInvalid {
        #[source]
        source: BoxError,
    },

    #[error("product user state is missing for product {product_listing_id}")]
    UserStateMissing {
        product_listing_id: ProductListingId,
    },
    #[error("product listing view URL could not be constructed")]
    ViewUrlInvalid {
        #[source]
        source: BoxError,
    },
    #[error("hidden product summary could not be constructed")]
    HiddenProductListingSummaryInvalid {
        #[source]
        source: BoxError,
    },
}

pub(crate) fn listing_source_ids(products: &[ProductListingSearchItem]) -> Vec<ListingSourceId> {
    products
        .iter()
        .map(|product| product.listing_source_id)
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn product_listing_ids(products: &[ProductListingSearchItem]) -> Vec<ProductListingId> {
    products
        .iter()
        .map(|product| product.product_listing_id)
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn product_listing_user_state_lookup(
    user_id: UserId,
    product_listing_ids: &[ProductListingId],
) -> ProductListingUserStateLookup {
    ProductListingUserStateLookup {
        user_id,
        product_listing_ids: product_listing_ids.to_vec(),
    }
}

pub(crate) async fn hydrate_listing_source_summaries<L>(
    products: Vec<ProductListingSearchItem>,
    listing_sources: &L,
) -> Result<Vec<ProductListingSearchItemWithSource>, ProductListingSummaryPersonalizationError>
where
    L: ListingSourceSummaryReader,
{
    if products.is_empty() {
        return Ok(Vec::new());
    }

    let listing_source_ids = listing_source_ids(&products);
    let summaries = listing_sources
        .find_summaries(&listing_source_ids)
        .await
        .map_err(ProductListingSummaryPersonalizationError::from)?;

    attach_listing_sources(products, &summaries)
}

pub(crate) fn attach_listing_sources(
    products: Vec<ProductListingSearchItem>,
    summaries: &HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
) -> Result<Vec<ProductListingSearchItemWithSource>, ProductListingSummaryPersonalizationError> {
    products
        .into_iter()
        .map(|item| {
            let listing_source_id = item.listing_source_id;
            let source = summaries.get(&listing_source_id).cloned().ok_or(
                ProductListingSummaryPersonalizationError::ListingSourceSummaryMissing {
                    listing_source_id,
                },
            )?;
            let view_url = outbound_url(source.referral_configuration.as_ref(), &item.url)
                .map_err(
                    |source| ProductListingSummaryPersonalizationError::ViewUrlInvalid {
                        source: box_error(source),
                    },
                )?;
            Ok(ProductListingSearchItemWithSource {
                item,
                source: source.summary,
                view_url,
            })
        })
        .collect()
}

pub(crate) async fn hydrate_product_search_items<U>(
    products: &mut [PersonalizedProductListingSearchItem],
    user_id: UserId,
    user_states: &U,
) -> Result<(), ProductListingSummaryPersonalizationError>
where
    U: ProductListingUserStateReader,
{
    if products.is_empty() {
        return Ok(());
    }

    let product_listing_ids = products
        .iter()
        .map(|product| product.item.item.product_listing_id)
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let lookup = product_listing_user_state_lookup(user_id, &product_listing_ids);
    let user_states = user_states
        .find_for_user(&lookup)
        .await
        .map_err(ProductListingSummaryPersonalizationError::from)?;

    apply_product_user_states(products, &user_states)
}

pub(crate) fn apply_product_user_states(
    products: &mut [PersonalizedProductListingSearchItem],
    user_states: &HashMap<ProductListingId, crate::user_state::ProductListingUserState>,
) -> Result<(), ProductListingSummaryPersonalizationError> {
    for product in products {
        let user_state = user_states
            .get(&product.item.item.product_listing_id)
            .cloned()
            .ok_or(
                ProductListingSummaryPersonalizationError::UserStateMissing {
                    product_listing_id: product.item.item.product_listing_id,
                },
            )?;

        let hidden = user_state.search_filter.hidden;
        product.user_state = Some(user_state);
        if hidden {
            redact_hidden_product_search_item(&mut product.item)?;
        }
    }

    Ok(())
}

fn redact_hidden_product_search_item(
    product: &mut ProductListingSearchItemWithSource,
) -> Result<(), ProductListingSummaryPersonalizationError> {
    let language = product
        .item
        .title
        .as_ref()
        .map(|title| title.localization)
        .unwrap_or(Language::En);
    let hidden_url = Url::parse("https://aura-historia.com/pricing").map_err(|source| {
        ProductListingSummaryPersonalizationError::HiddenProductListingSummaryInvalid {
            source: box_error(source),
        }
    })?;

    let listing_source_id = ListingSourceId::new();
    product.item.event_id = EventId::new();
    product.item.listing_source_id = listing_source_id;
    product.source = ListingSourceSummary {
        listing_source_id,
        name: ListingSourceName::try_from("Hidden").map_err(|source| {
            ProductListingSummaryPersonalizationError::HiddenProductListingSummaryInvalid {
                source: box_error(source),
            }
        })?,
        slug_id: ListingSourceSlugId::raw("hidden").map_err(|source| {
            ProductListingSummaryPersonalizationError::HiddenProductListingSummaryInvalid {
                source: box_error(source),
            }
        })?,
    };
    product.item.source_listing_id =
        SourceListingId::try_from("00000000-0000-0000-0000-000000000000").map_err(|error| {
            ProductListingSummaryPersonalizationError::HiddenProductListingSummaryInvalid {
                source: box_error(error),
            }
        })?;
    product.item.title = Some(Localized::new(language, hidden_title(language)));
    product.item.display_price = None;
    product.item.availability = None;
    product.item.lifecycle = ListingLifecycle::Active;
    product.item.url = hidden_url.clone();
    product.view_url = hidden_url;
    product.item.images.clear();
    product.item.updated = OffsetDateTime::UNIX_EPOCH;

    Ok(())
}

fn hidden_title(language: Language) -> Title {
    match language {
        Language::De => Title::from("Versteckter Produkttitel"),
        Language::En => Title::from("Hidden ProductListing Title"),
        Language::Fr => Title::from("Titre du produit masqué"),
        Language::Es => Title::from("Título de producto oculto"),
        Language::It => Title::from("Titolo del prodotto nascosto"),
        _ => Title::from("Hidden ProductListing Title"),
    }
}

impl From<ListingSourceSummaryReadError> for ProductListingSummaryPersonalizationError {
    fn from(error: ListingSourceSummaryReadError) -> Self {
        match error {
            ListingSourceSummaryReadError::QueryFailed { source } => {
                Self::ListingSourceSummaryQueryFailed { source }
            }
            ListingSourceSummaryReadError::InvalidReadModel { source } => {
                Self::ListingSourceSummaryReadModelInvalid { source }
            }
        }
    }
}

impl From<ProductListingUserStateReadError> for ProductListingSummaryPersonalizationError {
    fn from(error: ProductListingUserStateReadError) -> Self {
        match error {
            ProductListingUserStateReadError::QueryFailed { source } => {
                Self::UserStateQueryFailed { source }
            }
            ProductListingUserStateReadError::InvalidReadModel { source } => {
                Self::UserStateReadModelInvalid { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_cases::queries::search_product_listings::ProductListingSummaryPriceValuation;
    use indexmap::IndexSet;
    use money::{Currency, MonetaryAmount, Price};
    use product_listing_core::{
        listing_availability::ListingAvailability, listing_lifecycle::ListingLifecycle,
        product_listing_image::ProductListingImage,
    };
    use std::{collections::HashMap, sync::Mutex};

    struct RecordingListingSourceSummaryReader {
        summaries: HashMap<ListingSourceId, crate::ports::ListingSourceSummaryWithReferral>,
        requests: Mutex<Vec<Vec<ListingSourceId>>>,
    }

    #[async_trait::async_trait]
    impl ListingSourceSummaryReader for RecordingListingSourceSummaryReader {
        async fn find_summaries(
            &self,
            listing_source_ids: &[ListingSourceId],
        ) -> Result<
            HashMap<ListingSourceId, crate::ports::ListingSourceSummaryWithReferral>,
            ListingSourceSummaryReadError,
        > {
            match self.requests.lock() {
                Ok(mut requests) => requests.push(listing_source_ids.to_vec()),
                Err(poisoned) => poisoned.into_inner().push(listing_source_ids.to_vec()),
            }
            Ok(self.summaries.clone())
        }
    }

    fn search_item(listing_source_id: ListingSourceId) -> ProductListingSearchItem {
        ProductListingSearchItem {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("cabinet-a1b2c3")
                .unwrap_or_else(|error| panic!("valid product listing title slug: {error}")),
            event_id: EventId::new(),
            listing_source_id,
            source_listing_id: SourceListingId::try_from("cabinet-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            auction_id: None,
            has_auction_context: false,
            title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
            display_price: Some(
                product_listing_core::product_listing_price::ProductListingPrice::from(Price::new(
                    MonetaryAmount::from(100_u64),
                    Currency::Eur,
                )),
            ),
            price_valuation: ProductListingSummaryPriceValuation::Current {
                fx_rate_id: fxrate_core::FxRateId::new(),
                captured_at: OffsetDateTime::UNIX_EPOCH,
            },
            availability: Some(ListingAvailability::Available),
            lifecycle: ListingLifecycle::Active,
            url: Url::parse("https://source.example/cabinet")
                .unwrap_or_else(|error| panic!("valid source URL: {error}")),
            images: IndexSet::<ProductListingImage>::new(),
            updated: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[tokio::test]
    async fn should_hydrate_unique_listing_sources_once_and_preserve_search_order() {
        let source_one = ListingSourceId::new();
        let source_two = ListingSourceId::new();
        let expected_order = [source_one, source_two, source_one];
        let reader = RecordingListingSourceSummaryReader {
            summaries: HashMap::from([
                (
                    source_one,
                    crate::ports::ListingSourceSummaryWithReferral {
                        summary: ListingSourceSummary {
                            listing_source_id: source_one,
                            name: ListingSourceName::try_from("One").unwrap_or_else(|error| {
                                panic!("invalid test listing source name: {error}")
                            }),
                            slug_id: ListingSourceSlugId::raw("one").unwrap_or_else(|error| {
                                panic!("valid test listing source slug: {error}")
                            }),
                        },
                        referral_configuration: Some(
                            listing_source_core::ReferralConfiguration::Partnerize {
                                camref: listing_source_core::PartnerizeCamref::try_from("campaign")
                                    .unwrap_or_else(|error| panic!("test camref: {error}")),
                            },
                        ),
                    },
                ),
                (
                    source_two,
                    crate::ports::ListingSourceSummaryWithReferral {
                        summary: ListingSourceSummary {
                            listing_source_id: source_two,
                            name: ListingSourceName::try_from("Two").unwrap_or_else(|error| {
                                panic!("invalid test listing source name: {error}")
                            }),
                            slug_id: ListingSourceSlugId::raw("two").unwrap_or_else(|error| {
                                panic!("valid test listing source slug: {error}")
                            }),
                        },
                        referral_configuration: None,
                    },
                ),
            ]),
            requests: Mutex::new(Vec::new()),
        };

        let products = hydrate_listing_source_summaries(
            expected_order.into_iter().map(search_item).collect(),
            &reader,
        )
        .await;

        let products = match products {
            Ok(products) => products,
            Err(error) => panic!("source hydration must succeed: {error}"),
        };
        assert_eq!(
            vec![source_one, source_two, source_one],
            products
                .iter()
                .map(|product| product.source.listing_source_id)
                .collect::<Vec<_>>()
        );
        assert!(
            products[0]
                .view_url
                .as_str()
                .starts_with("https://prf.hn/click/camref:campaign/")
        );
        assert!(
            products[1]
                .view_url
                .as_str()
                .contains("utm_source=aura_historia")
        );
        let requests = match reader.requests.lock() {
            Ok(requests) => requests,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert_eq!(vec![vec![source_one, source_two]], *requests);
    }

    #[tokio::test]
    async fn should_fail_when_a_search_listing_source_summary_is_missing() {
        let listing_source_id = ListingSourceId::new();
        let reader = RecordingListingSourceSummaryReader {
            summaries: HashMap::new(),
            requests: Mutex::new(Vec::new()),
        };

        let result =
            hydrate_listing_source_summaries(vec![search_item(listing_source_id)], &reader).await;

        assert!(matches!(
            result,
            Err(ProductListingSummaryPersonalizationError::ListingSourceSummaryMissing {
                listing_source_id: missing,
            }) if missing == listing_source_id
        ));
    }
}

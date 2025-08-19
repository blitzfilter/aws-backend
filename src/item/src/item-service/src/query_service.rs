use async_trait::async_trait;
use common::language::domain::Language;
use common::page::Page;
use common::price::domain::Price;
use common::sort::Sort;
use common::{currency::domain::Currency, localized::Localized};
use item_core::hash::ItemHash;
use item_core::{
    description::Description,
    item::{LocalizedItemView, SortItemField},
    title::Title,
};
use item_opensearch::repository::ItemOpenSearchRepository;
use search_filter_core::search_filter::SearchFilter;
use std::collections::HashMap;
use tracing::{error, warn};

#[derive(thiserror::Error, Debug)]
pub enum SearchItemsError {
    #[error("OpenSearchError: {0}")]
    OpenSearchError(#[from] opensearch::Error),
}

#[async_trait]
#[mockall::automock]
pub trait QueryItemService {
    async fn search_items(
        &self,
        search_filter: &SearchFilter,
        language: &Language,
        currency: &Currency,
        sort: &Option<Sort<SortItemField>>,
        page: &Option<Page>,
    ) -> Result<Vec<LocalizedItemView>, SearchItemsError>;
}

pub struct QueryItemServiceImpl<'a> {
    repository: &'a (dyn ItemOpenSearchRepository + Sync),
}

impl<'a> QueryItemServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ItemOpenSearchRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<'a> QueryItemService for QueryItemServiceImpl<'a> {
    async fn search_items(
        &self,
        search_filter: &SearchFilter,
        language: &Language,
        currency: &Currency,
        sort: &Option<Sort<SortItemField>>,
        page: &Option<Page>,
    ) -> Result<Vec<LocalizedItemView>, SearchItemsError> {
        let search_response = self
            .repository
            .search_item_documents(search_filter, language, currency, sort, page)
            .await?;

        if search_response.timed_out {
            warn!(
                searchFilter = ?search_filter,
                language = ?language,
                currency = %currency,
                sort = ?sort,
                page = ?page,
                took = search_response.took,
                shard_stats = ?search_response.shards,
                "Search-Request to OpenSearch timed out when querying items."
            );
        }

        let item_views = search_response.hits.hits.into_iter().map(|hit| hit.source).map(|item_document| {
            let mut available_titles: HashMap<Language, Title> = HashMap::with_capacity(3);
            if let Some(title_de) = item_document.title_de {
                available_titles.insert(Language::De, title_de.into());
            }
            if let Some(title_en) = item_document.title_en {
                available_titles.insert(Language::En, title_en.into());
            }

            let mut available_descriptions: HashMap<Language, Description> = HashMap::with_capacity(3);
            if let Some(description_de) = item_document.description_de {
                available_descriptions.insert(Language::De, description_de.into());
            }
            if let Some(description_en) = item_document.description_en {
                available_descriptions.insert(Language::En, description_en.into());
            }

            let title = Language::resolve(&[*language], available_titles).unwrap_or_else(|| {
                error!(
                    shopId = %item_document.shop_id,
                    shopsItemId = %item_document.shops_item_id,
                    "Failed resolving title. This SHOULD be impossible because the native title always exists."
                );
                Localized::new(Language::En, "Unknown title".into())
            });
            let description = Language::resolve(&[*language], available_descriptions);

            let price = match currency {
                Currency::Eur => item_document
                    .price_eur
                    .map(|amount| Price::new(amount.into(), Currency::Eur)),
                Currency::Gbp => item_document
                    .price_gbp
                    .map(|amount| Price::new(amount.into(), Currency::Gbp)),
                Currency::Usd => item_document
                    .price_usd
                    .map(|amount| Price::new(amount.into(), Currency::Usd)),
                Currency::Aud => item_document
                    .price_aud
                    .map(|amount| Price::new(amount.into(), Currency::Aud)),
                Currency::Cad => item_document
                    .price_cad
                    .map(|amount| Price::new(amount.into(), Currency::Cad)),
                Currency::Nzd => item_document
                    .price_nzd
                    .map(|amount| Price::new(amount.into(), Currency::Nzd)),
            };
            let state = item_document.state.into();

            LocalizedItemView {
                item_id: item_document.item_id,
                event_id: item_document.event_id,
                shop_id: item_document.shop_id,
                shops_item_id: item_document.shops_item_id,
                shop_name: item_document.shop_name.into(),
                title,
                description,
                price,
                state,
                url: item_document.url,
                images: item_document.images,
                hash: ItemHash::new(&price, &state),
                created: item_document.created,
                updated: item_document.updated,
            }
        })
        .collect::<Vec<_>>();

        Ok(item_views)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::query_service::{QueryItemService, QueryItemServiceImpl};
    use common::{
        currency::domain::Currency,
        item_state::domain::ItemState,
        language::domain::Language,
        opensearch::search_response::{
            HitsMetadata, SearchHit, SearchResponse, ShardStats, TotalHits,
        },
        page::Page,
        sort::{Sort, SortDirection},
    };
    use item_core::item::SortItemField;
    use item_opensearch::{item_document::ItemDocument, repository::MockItemOpenSearchRepository};
    use search_filter_core::{
        array_query::AnyOfQuery, range_query::RangeQuery, search_filter::SearchFilter,
    };
    use time::macros::datetime;

    fn mk_search_response(item_documents: Vec<ItemDocument>) -> SearchResponse<ItemDocument> {
        SearchResponse {
            took: 42,
            timed_out: false,
            shards: ShardStats {
                total: 5,
                successful: 4,
                skipped: 1,
                failed: 0,
            },
            hits: HitsMetadata {
                total: TotalHits {
                    value: item_documents.len() as u64,
                    relation: "eq".to_string(),
                },
                max_score: None,
                hits: item_documents
                    .into_iter()
                    .map(|item_document| SearchHit {
                        index: "items".to_string(),
                        id: item_document.item_id.to_string(),
                        score: None,
                        source: item_document,
                    })
                    .collect(),
            },
        }
    }

    #[rstest::rstest]
    #[case(
        SearchFilter {
            item_query: "Hallo Welt".into(),
            shop_name_query: Some("Hallo Shop".into()),
            price_query: Some(RangeQuery { min: Some(100u64.into()), max: Some(999999u64.into()) }),
            state_query: AnyOfQuery(HashSet::from_iter([ItemState::Available, ItemState::Listed])),
            created_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: Some(datetime!(3000-01-01 0:00 UTC)) }),
            updated_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: Some(datetime!(3000-01-01 0:00 UTC)) }),
        },
        Language::De,
        Currency::Eur,
        Some(Sort { field: SortItemField::Price, direction: SortDirection::Asc }),
        Some(Page { from: 0, size: 20 }),
        100
    )]
    #[case(
        SearchFilter {
            item_query: "Hallo Welt".into(),
            shop_name_query: Some("Hallo Shop".into()),
            price_query: Some(RangeQuery { min: Some(100u64.into()), max: Some(999999u64.into()) }),
            state_query: AnyOfQuery(HashSet::from_iter([ItemState::Available, ItemState::Listed])),
            created_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: Some(datetime!(3000-01-01 0:00 UTC)) }),
            updated_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: Some(datetime!(3000-01-01 0:00 UTC)) }),
        },
        Language::En,
        Currency::Usd,
        Some(Sort { field: SortItemField::Price, direction: SortDirection::Desc }),
        Some(Page { from: 10, size: 30 }),
        500
    )]
    #[case(
        SearchFilter {
            item_query: "Hallo Welten!".into(),
            shop_name_query: None,
            price_query: Some(RangeQuery { min: Some(100000u64.into()), max: Some(999999004u64.into()) }),
            state_query: AnyOfQuery(HashSet::from_iter([ItemState::Available, ItemState::Listed])),
            created_query: Some(RangeQuery { min: None, max: Some(datetime!(3000-01-01 0:00 UTC)) }),
            updated_query: Some(RangeQuery { min: Some(datetime!(1000-01-01 0:00 UTC)), max: None }),
        },
        Language::En,
        Currency::Gbp,
        None,
        None,
        1111
    )]
    #[case(
        SearchFilter {
            item_query: "Hallo Welten!".into(),
            shop_name_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
        },
        Language::Fr,
        Currency::Eur,
        None,
        None,
        123
    )]
    #[case(
        SearchFilter {
            item_query: "Hallo Welten!".into(),
            shop_name_query: None,
            price_query: None,
            state_query: Default::default(),
            created_query: None,
            updated_query: None,
        },
        Language::Es,
        Currency::Eur,
        None,
        None,
        1234
    )]
    #[tokio::test]
    async fn should_search_items(
        #[case] search_filter: SearchFilter,
        #[case] language: Language,
        #[case] currency: Currency,
        #[case] sort: Option<Sort<SortItemField>>,
        #[case] page: Option<Page>,
        #[case] count: usize,
    ) {
        let mut repository = MockItemOpenSearchRepository::default();
        repository
            .expect_search_item_documents()
            .return_once(move |_, _, _, _, _| {
                Box::pin(async move { Ok(mk_search_response(fake::vec![ItemDocument; count])) })
            });
        let service = QueryItemServiceImpl::new(&repository);

        let actual = service
            .search_items(&search_filter, &language, &currency, &sort, &page)
            .await
            .unwrap();

        assert_eq!(count, actual.len());
    }
}

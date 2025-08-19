use async_trait::async_trait;
use common::currency::domain::Currency;
use common::language::domain::Language;
use common::page::Page;
use common::sort::Sort;
use item_core::item::{LocalizedItemView, SortItemField};
use item_opensearch::repository::ItemOpenSearchRepository;
use search_filter_core::search_filter::SearchFilter;

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
        let _documents = self
            .repository
            .search_item_documents(search_filter, language, currency, sort, page)
            .await?;

        todo!()
    }
}

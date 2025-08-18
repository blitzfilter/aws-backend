use crate::{array_query::ArrayQuery, range_query::RangeQuery, text_query::TextQuery};
use common::item_state::domain::ItemState;
use common::price::domain::MonetaryAmount;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilter {
    pub item_query: TextQuery,
    pub shop_name_query: Option<TextQuery>,
    pub price_query: Option<RangeQuery<MonetaryAmount>>,
    pub state_query: ArrayQuery<ItemState>,
    pub created_query: Option<RangeQuery<OffsetDateTime>>,
    pub updated_query: Option<RangeQuery<OffsetDateTime>>,
}

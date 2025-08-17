use crate::{array_query::ArrayQuery, range_query::RangeQuery, text_query::TextQuery};
use common::item_state::domain::ItemState;
use common::price::domain::MonetaryAmount;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilter {
    item_query: TextQuery,
    shop_name_query: Option<TextQuery>,
    price_query: Option<RangeQuery<MonetaryAmount>>,
    state_query: ArrayQuery<ItemState>,
    created_query: Option<RangeQuery<OffsetDateTime>>,
    updated_query: Option<RangeQuery<OffsetDateTime>>,
}

use crate::{array_query::ArrayQuery, range_query::RangeQuery, text_query::TextQuery};
use common::{currency::domain::Currency, price::domain::MonetaryAmount};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFilter {
    item_query: TextQuery,
    shop_name_query: Option<TextQuery>,
    price_query: Option<MonetaryAmount>,
    state_query: ArrayQuery<ItemState>,
    is_available: Option<bool>,
    created_query: Option<RangeQuery<OffsetDateTime>>,
    updated_query: Option<RangeQuery<OffsetDateTime>>,
}

pub struct PriceQuery {
    pub range: RangeQuery<MonetaryAmount>,
    pub currency: Currency,
}

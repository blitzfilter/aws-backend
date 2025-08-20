#[cfg_attr(feature = "api", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "api", serde(rename_all = "lowercase"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortOrder::Asc => "asc",
            SortOrder::Desc => "desc",
        }
    }
}

impl From<SortOrder> for &'static str {
    fn from(value: SortOrder) -> Self {
        value.as_str()
    }
}

impl<'a> TryFrom<&'a str> for SortOrder {
    type Error = String;

    fn try_from(value: &'a str) -> Result<Self, Self::Error> {
        match value {
            "asc" => Ok(SortOrder::Asc),
            "desc" => Ok(SortOrder::Desc),
            invalid => Err(format!("Expected any of: 'asc', 'desc'. Got: '{invalid}'")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sort<T> {
    pub sort: T,
    pub order: SortOrder,
}

impl<T> Sort<T> {
    pub fn map<U, F>(self, f: F) -> Sort<U>
    where
        F: FnOnce(T) -> U,
    {
        Sort {
            sort: f(self.sort),
            order: self.order,
        }
    }
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_ORDER_VALUE, BAD_SORT_VALUE},
        },
        sort::{Sort, SortOrder},
    };
    use aws_lambda_events::query_map::QueryMap;

    pub fn extract_sort_query<'a, T: TryFrom<&'a str, Error = impl Into<String>>>(
        headers: &'a QueryMap,
    ) -> Result<Option<Sort<T>>, ApiError> {
        let sort = headers
            .first("sort")
            .map(str::trim)
            .map(T::try_from)
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(BAD_SORT_VALUE)
                    .with_query_field("sort")
                    .with_message(err)
            })?;
        let order = headers
            .first("order")
            .map(str::trim)
            .map(SortOrder::try_from)
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(BAD_ORDER_VALUE)
                    .with_query_field("order")
                    .with_message(err)
            })?;

        if let Some(sort) = sort
            && let Some(order) = order
        {
            Ok(Some(Sort { sort, order }))
        } else {
            Ok(None)
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::api::error::{ApiErrorSource, ApiErrorSourceType};
        use crate::api::error_code::{BAD_ORDER_VALUE, BAD_SORT_VALUE};
        use crate::sort::api::extract_sort_query;
        use crate::sort::{Sort, SortOrder};
        use aws_lambda_events::query_map::QueryMap;
        use serde::{Deserialize, Serialize};
        use std::collections::HashMap;

        #[rstest::rstest]
        #[case(SortOrder::Asc)]
        #[case(SortOrder::Desc)]
        fn should_match_as_str_serialize(#[case] field: SortOrder) {
            let serialized = serde_json::to_string(&field).unwrap().replace("\"", "");
            let as_str = field.as_str();

            assert_eq!(as_str, &serialized);
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum DummyField {
            Foo,
            Bar,
        }

        impl<'a> TryFrom<&'a str> for DummyField {
            type Error = String;

            fn try_from(value: &'a str) -> Result<Self, Self::Error> {
                match value {
                    "foo" => Ok(DummyField::Foo),
                    "bar" => Ok(DummyField::Bar),
                    invalid => Err(format!("Expected any of: 'foo', 'bar'. Got: '{invalid}'")),
                }
            }
        }

        #[rstest::rstest]
        #[case(Some("foo"), Some("asc"), Some(Sort { sort: DummyField::Foo, order: SortOrder::Asc }))]
        #[case(Some("foo"), Some("desc"), Some(Sort { sort: DummyField::Foo, order: SortOrder::Desc }))]
        #[case(Some("bar"), Some("asc"), Some(Sort { sort: DummyField::Bar, order: SortOrder::Asc }))]
        #[case(Some("bar"), Some("desc"), Some(Sort { sort: DummyField::Bar, order: SortOrder::Desc }))]
        #[case(None, Some("asc"), None)]
        #[case(None, Some("desc"), None)]
        #[case(Some("foo"), None, None)]
        #[case(None, None, None)]
        fn should_extract_sort(
            #[case] sort_value: Option<&str>,
            #[case] order_value: Option<&str>,
            #[case] expected: Option<Sort<DummyField>>,
        ) {
            let mut map = HashMap::new();
            if let Some(sort_value) = sort_value {
                map.insert("sort".to_string(), sort_value.to_string());
            }
            if let Some(order_value) = order_value {
                map.insert("order".to_string(), order_value.to_string());
            }
            let query = QueryMap::from(map);

            let actual = extract_sort_query(&query).unwrap();

            assert_eq!(expected, actual);
        }

        #[rstest::rstest]
        #[case("boop")]
        #[case("baz")]
        #[case("fooo")]
        #[case("bart")]
        #[case("1x")]
        #[case("07g")]
        #[case("65536")]
        fn should_400_when_sort_is_invalid(#[case] value: &str) {
            let mut map = HashMap::new();
            map.insert("sort".to_string(), value.to_string());
            let query = QueryMap::from(map);

            let actual = extract_sort_query::<DummyField>(&query).unwrap_err();

            assert_eq!(400, actual.status);
            assert_eq!(BAD_SORT_VALUE, actual.error);
            assert_eq!(
                Some(ApiErrorSource {
                    field: "sort",
                    source_type: ApiErrorSourceType::Query,
                }),
                actual.source
            )
        }

        #[rstest::rstest]
        #[case("asci")]
        #[case("descendent")]
        #[case("boop")]
        #[case("baz")]
        #[case("fooo")]
        #[case("bart")]
        #[case("1x")]
        #[case("07g")]
        #[case("65536")]
        fn should_400_when_order_is_invalid(#[case] value: &str) {
            let mut map = HashMap::new();
            map.insert("order".to_string(), value.to_string());
            let query = QueryMap::from(map);

            let actual = extract_sort_query::<DummyField>(&query).unwrap_err();

            assert_eq!(400, actual.status);
            assert_eq!(BAD_ORDER_VALUE, actual.error);
            assert_eq!(
                Some(ApiErrorSource {
                    field: "order",
                    source_type: ApiErrorSourceType::Query,
                }),
                actual.source
            )
        }
    }
}

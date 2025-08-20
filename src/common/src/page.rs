#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub from: u16,
    pub size: u16,
}

#[cfg(feature = "api")]
pub mod api {
    use crate::{
        api::{
            error::ApiError,
            error_code::{BAD_PAGE_FROM_VALUE, BAD_PAGE_SIZE_VALUE},
        },
        page::Page,
    };
    use aws_lambda_events::query_map::QueryMap;

    pub fn extract_page_query(headers: &QueryMap) -> Result<Option<Page>, ApiError> {
        let from = headers
            .first("from")
            .map(str::trim)
            .map(|from| from.parse::<u16>())
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(BAD_PAGE_FROM_VALUE)
                    .with_query_field("from")
                    .with_message(err.to_string())
            })?;
        let size = headers
            .first("size")
            .map(str::trim)
            .map(|size| size.parse::<u16>())
            .transpose()
            .map_err(|err| {
                ApiError::bad_request(BAD_PAGE_SIZE_VALUE)
                    .with_query_field("size")
                    .with_message(err.to_string())
            })?;

        if let Some(from) = from
            && let Some(size) = size
        {
            Ok(Some(Page { from, size }))
        } else {
            Ok(None)
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::api::error::{ApiErrorSource, ApiErrorSourceType};
        use crate::api::error_code::{BAD_PAGE_FROM_VALUE, BAD_PAGE_SIZE_VALUE};
        use crate::page::Page;
        use crate::page::api::extract_page_query;
        use aws_lambda_events::query_map::QueryMap;
        use std::collections::HashMap;

        #[rstest::rstest]
        #[case(Some("0"), Some("10"), Some(Page { from: 0, size: 10 }))]
        #[case(Some("10"), Some("10"), Some(Page { from: 10, size: 10 }))]
        #[case(Some("42"), Some("69"), Some(Page { from: 42, size: 69 }))]
        #[case(Some("69"), Some("37"), Some(Page { from: 69, size: 37 }))]
        #[case(Some(" 69 "), Some(" 37 "), Some(Page { from: 69, size: 37 }))]
        #[case(Some(" 69"), Some(" 37"), Some(Page { from: 69, size: 37 }))]
        #[case(Some("1"), Some("1"), Some(Page { from: 1, size: 1 }))]
        #[case(Some("7"), Some("65535"), Some(Page { from: 7, size: 65535 }))]
        #[case(None, Some("1"), None)]
        #[case(None, Some("42"), None)]
        #[case(Some("31"), None, None)]
        #[case(None, None, None)]
        fn should_extract_page(
            #[case] from_value: Option<&str>,
            #[case] size_value: Option<&str>,
            #[case] expected: Option<Page>,
        ) {
            let mut map = HashMap::new();
            if let Some(from_value) = from_value {
                map.insert("from".to_string(), from_value.to_string());
            }
            if let Some(size_value) = size_value {
                map.insert("size".to_string(), size_value.to_string());
            }
            let query = QueryMap::from(map);

            let actual = extract_page_query(&query).unwrap();

            assert_eq!(expected, actual);
        }

        #[rstest::rstest]
        #[case("boop")]
        #[case("foo")]
        #[case("bar")]
        #[case("1x")]
        #[case("07g")]
        #[case("65536")]
        fn should_400_when_from_is_invalid(#[case] value: &str) {
            let mut map = HashMap::new();
            map.insert("from".to_string(), value.to_string());
            let query = QueryMap::from(map);

            let actual = extract_page_query(&query).unwrap_err();

            assert_eq!(400, actual.status);
            assert_eq!(BAD_PAGE_FROM_VALUE, actual.error);
            assert_eq!(
                Some(ApiErrorSource {
                    field: "from",
                    source_type: ApiErrorSourceType::Query,
                }),
                actual.source
            )
        }

        #[rstest::rstest]
        #[case("boop")]
        #[case("foo")]
        #[case("bar")]
        #[case("1x")]
        #[case("07g")]
        #[case("65536")]
        fn should_400_when_size_is_invalid(#[case] value: &str) {
            let mut map = HashMap::new();
            map.insert("size".to_string(), value.to_string());
            let query = QueryMap::from(map);

            let actual = extract_page_query(&query).unwrap_err();

            assert_eq!(400, actual.status);
            assert_eq!(BAD_PAGE_SIZE_VALUE, actual.error);
            assert_eq!(
                Some(ApiErrorSource {
                    field: "size",
                    source_type: ApiErrorSourceType::Query,
                }),
                actual.source
            )
        }
    }
}

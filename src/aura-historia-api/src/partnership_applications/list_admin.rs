use super::{types::AdminPartnershipApplicationSummaryData, util::no_store};
use crate::{
    auth::protected_context,
    error::{
        ApiError, BAD_ORDER_VALUE, BAD_QUERY_PARAMETER_VALUE, BAD_SORT_VALUE,
        PARTNERSHIP_APPLICATION_INTERNAL_ERROR,
    },
    pagination_data::JsonCursoredData,
    state::PartnershipApplicationsState,
    wire::parse_query_object_id,
};
use application::pagination::Cursor;
use axum::{
    Json,
    extract::{RawQuery, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use domain_primitives::{
    query::{any_of_query::AnyOfQuery, range_query::RangeQuery},
    sort::{Sort, SortOrder},
};
use listing_source_core::ListingSourceId;
use partnership_core::{
    partnership_application_search::PartnershipApplicationSearch,
    partnership_application_state::PartnershipApplicationState,
    partnership_proposal_type::PartnershipProposalType,
    sort_partnership_application_field::SortPartnershipApplicationField,
};
use partnership_service::use_cases::queries::list_admin_partnership_applications::{
    ListAdminPartnershipApplicationsRequest, PartnershipApplicationSearchCursor,
};
use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user_core::user_id::UserId;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListAdminPartnershipApplicationsQuery {
    #[serde(default)]
    state: Vec<String>,
    #[serde(default)]
    applicant_user_id: Option<String>,
    #[serde(default)]
    proposal_type: Vec<String>,
    #[serde(default)]
    listing_source_id: Option<String>,
    #[serde(
        default,
        with = "domain_primitives::query::range_query::range_rfc3339::option"
    )]
    created: Option<RangeQuery<OffsetDateTime>>,
    #[serde(
        default,
        with = "domain_primitives::query::range_query::range_rfc3339::option"
    )]
    updated: Option<RangeQuery<OffsetDateTime>>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

pub(super) async fn list_admin(
    State(state): State<PartnershipApplicationsState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let request = match parse_list_admin_query(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };

    match state.list_admin.execute(&context, request).await {
        Ok(result) => {
            let search_after = match result
                .cursor
                .search_after
                .map(admin_cursor_value)
                .transpose()
            {
                Ok(value) => value,
                Err(error) => return no_store(error.into_response()),
            };
            let response = Json(JsonCursoredData {
                items: result
                    .items
                    .into_iter()
                    .map(AdminPartnershipApplicationSummaryData::from)
                    .collect(),
                size: result.cursor.size,
                search_after,
                total: result.total,
            })
            .into_response();
            no_store(response)
        }
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_list_admin_query(
    raw_query: Option<&str>,
) -> Result<ListAdminPartnershipApplicationsRequest, ApiError> {
    let query: ListAdminPartnershipApplicationsQuery = serde_qs::Config::new()
        .use_form_encoding(true)
        .deserialize_str(raw_query.unwrap_or_default())
        .map_err(|error| bad_query("query", error))?;

    let search = PartnershipApplicationSearch {
        state_query: parse_states(query.state)?,
        applicant_user_id: query
            .applicant_user_id
            .map(|value| parse_user_id(&value))
            .transpose()?,
        proposal_type_query: parse_proposal_types(query.proposal_type)?,
        listing_source_id: query
            .listing_source_id
            .map(|value| parse_listing_source_id(&value))
            .transpose()?,
        created: query.created,
        updated: query.updated,
    };
    let sort = parse_sort(query.sort.as_deref(), query.order.as_deref())?;
    let cursor = parse_cursor(query.size.as_deref(), query.search_after.as_deref())?;

    Ok(ListAdminPartnershipApplicationsRequest {
        search,
        sort,
        cursor,
    })
}

fn parse_states(values: Vec<String>) -> Result<AnyOfQuery<PartnershipApplicationState>, ApiError> {
    values
        .into_iter()
        .map(|value| {
            PartnershipApplicationState::from_code(&value)
                .ok_or_else(|| bad_query("state", format!("Unsupported state '{value}'.")))
        })
        .collect()
}

fn parse_proposal_types(
    values: Vec<String>,
) -> Result<AnyOfQuery<PartnershipProposalType>, ApiError> {
    values
        .into_iter()
        .map(|value| {
            PartnershipProposalType::from_code(&value).ok_or_else(|| {
                bad_query(
                    "proposalType",
                    format!("Unsupported proposal type '{value}'."),
                )
            })
        })
        .collect()
}

fn parse_user_id(value: &str) -> Result<UserId, ApiError> {
    parse_query_object_id(value, "applicantUserId", "User")
}

fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ApiError> {
    parse_query_object_id(value, "listingSourceId", "ListingSource")
}

fn parse_sort(
    sort: Option<&str>,
    order: Option<&str>,
) -> Result<Option<Sort<SortPartnershipApplicationField>>, ApiError> {
    let sort = sort
        .map(|value| match value {
            "created" => Ok(SortPartnershipApplicationField::Created),
            "updated" => Ok(SortPartnershipApplicationField::Updated),
            value => Err(ApiError::bad_request(BAD_SORT_VALUE)
                .with_query_field("sort")
                .with_detail(format!(
                    "Expected any of: 'created', 'updated'. Got: '{value}'"
                ))),
        })
        .transpose()?;
    let order = order
        .map(|value| {
            SortOrder::try_from(value).map_err(|detail| {
                ApiError::bad_request(BAD_ORDER_VALUE)
                    .with_query_field("order")
                    .with_detail(detail)
            })
        })
        .transpose()?;

    match (sort, order) {
        (None, None) => Ok(None),
        (Some(sort), Some(order)) => Ok(Some(Sort { sort, order })),
        (Some(_), None) => Err(ApiError::bad_request(BAD_ORDER_VALUE)
            .with_query_field("order")
            .with_detail("'order' is required when 'sort' is supplied.")),
        (None, Some(_)) => Err(ApiError::bad_request(BAD_SORT_VALUE)
            .with_query_field("sort")
            .with_detail("'sort' is required when 'order' is supplied.")),
    }
}

fn parse_cursor(
    size: Option<&str>,
    search_after: Option<&str>,
) -> Result<Option<Cursor<PartnershipApplicationSearchCursor>>, ApiError> {
    let size = size
        .map(|value| value.parse::<u64>().map(|size| size.clamp(1, 100)))
        .transpose()
        .map_err(|error| bad_query("size", error))?;
    let search_after = search_after.map(parse_search_after).transpose()?;

    if size.is_some() || search_after.is_some() {
        Ok(Some(Cursor {
            size: size.unwrap_or(21),
            search_after,
        }))
    } else {
        Ok(None)
    }
}

fn parse_search_after(value: &str) -> Result<PartnershipApplicationSearchCursor, ApiError> {
    let value: Value =
        serde_json::from_str(value).map_err(|error| bad_query("searchAfter", error))?;
    let Value::Array(values) = value else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must be a JSON array containing timestamp and application ID.",
        ));
    };
    let [Value::String(position), Value::String(application_id)] = values.as_slice() else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must contain an RFC3339 timestamp and PartnershipApplication ID.",
        ));
    };
    let position = OffsetDateTime::parse(position, &Rfc3339)
        .map_err(|error| bad_query("searchAfter", error))?;
    let application_id =
        parse_query_object_id(application_id, "searchAfter", "PartnershipApplication")?;
    Ok(PartnershipApplicationSearchCursor {
        position,
        application_id,
    })
}

fn admin_cursor_value(cursor: PartnershipApplicationSearchCursor) -> Result<Value, ApiError> {
    cursor
        .position
        .format(&Rfc3339)
        .map(|position| json!([position, cursor.application_id]))
        .map_err(|_| {
            ApiError::internal_server_error(PARTNERSHIP_APPLICATION_INTERNAL_ERROR)
                .with_detail("Partnership application cursor failed internally.")
        })
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_primitives::sort::SortOrder;
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use serde_json::json;
    use time::macros::datetime;

    #[test]
    fn should_map_admin_application_search_query() -> Result<(), ApiError> {
        let applicant_user_id = UserId::new();
        let listing_source_id = ListingSourceId::new();
        let application_id = PartnershipApplicationId::new();
        let request = parse_list_admin_query(Some(&format!(
            "state=SUBMITTED&state=IN_REVIEW&applicantUserId={applicant_user_id}&proposalType=EXISTING_LISTING_SOURCE&listingSourceId={listing_source_id}&created%5Bmin%5D=2026-01-01T00%3A00%3A00Z&created%5Bmax%5D=2026-12-31T23%3A59%3A59Z&updated%5Bmin%5D=2026-02-01T00%3A00%3A00Z&sort=updated&order=asc&size=200&searchAfter=%5B%222026-09-04T12%3A00%3A00Z%22%2C%22{application_id}%22%5D"
        )))?;

        assert!(
            request
                .search
                .state_query
                .contains(&PartnershipApplicationState::Submitted)
        );
        assert!(
            request
                .search
                .state_query
                .contains(&PartnershipApplicationState::InReview)
        );
        assert_eq!(Some(applicant_user_id), request.search.applicant_user_id);
        assert!(
            request
                .search
                .proposal_type_query
                .contains(&PartnershipProposalType::ExistingListingSource)
        );
        assert_eq!(Some(listing_source_id), request.search.listing_source_id);
        assert_eq!(
            Some(RangeQuery {
                min: Some(datetime!(2026-01-01 00:00 UTC)),
                max: Some(datetime!(2026-12-31 23:59:59 UTC)),
            }),
            request.search.created
        );
        assert_eq!(
            Some(RangeQuery {
                min: Some(datetime!(2026-02-01 00:00 UTC)),
                max: None,
            }),
            request.search.updated
        );
        assert_eq!(
            Some(Sort {
                sort: SortPartnershipApplicationField::Updated,
                order: SortOrder::Asc,
            }),
            request.sort
        );
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(PartnershipApplicationSearchCursor {
                    position: datetime!(2026-09-04 12:00 UTC),
                    application_id,
                }),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_use_default_sort_and_page_size() -> Result<(), ApiError> {
        let request = parse_list_admin_query(None)?;

        assert_eq!(None, request.sort);
        assert_eq!(None, request.cursor);
        assert!(request.search.state_query.is_empty());
        assert!(request.search.proposal_type_query.is_empty());
        Ok(())
    }

    #[test]
    fn should_reject_invalid_admin_application_query_values() {
        let cases = [
            ("state=invalid", "BAD_QUERY_PARAMETER_VALUE"),
            ("proposalType=invalid", "BAD_QUERY_PARAMETER_VALUE"),
            ("applicantUserId=not-an-object-id", "INVALID_OBJECT_ID"),
            ("listingSourceId=not-an-object-id", "INVALID_OBJECT_ID"),
            (
                "created%5Bmin%5D=not-a-timestamp",
                "BAD_QUERY_PARAMETER_VALUE",
            ),
            ("sort=invalid", "BAD_SORT_VALUE"),
            ("sort=invalid&order=asc", "BAD_SORT_VALUE"),
            ("sort=created&order=sideways", "BAD_ORDER_VALUE"),
            ("order=sideways", "BAD_ORDER_VALUE"),
            ("sort=created", "BAD_ORDER_VALUE"),
            ("size=not-a-number", "BAD_QUERY_PARAMETER_VALUE"),
            ("searchAfter=not-json", "BAD_QUERY_PARAMETER_VALUE"),
            (
                "searchAfter=%5B%22not-a-timestamp%22%2C%22not-an-object-id%22%5D",
                "BAD_QUERY_PARAMETER_VALUE",
            ),
        ];

        for (query, expected_error) in cases {
            let error = parse_list_admin_query(Some(query))
                .err()
                .unwrap_or_else(|| panic!("query should be rejected: {query}"));
            let body = serde_json::to_value(error)
                .unwrap_or_else(|error| panic!("failed to serialize API error: {error}"));
            assert_eq!(json!(expected_error), body["error"], "query: {query}");
        }
    }
}

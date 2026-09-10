use super::no_store;
use crate::auth::protected_context;
use crate::error::{ApiError, BAD_QUERY_PARAMETER_VALUE, OAUTH_INTERNAL_ERROR};
use crate::pagination_data::JsonCursoredData;
use crate::state::OAuthState;
use crate::wire::parse_query_object_id;
use application::pagination::{Cursor, CursoredResult};
use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::OAuthClientSearch;
use oauth_service::use_cases::{
    ListOAuthClientsRequest, ListOAuthClientsResult, OAuthClientSearchCursor,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

const MAX_PAGE_SIZE: u64 = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListOAuthClientsQuery {
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    search_after: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct OAuthClientAdminData {
    client_id: OAuthClientId,
    client_name: String,
    tos_uri: Url,
    policy_uri: Url,
    client_uri: Url,
    logo_uri: Url,
    redirect_uris: Vec<Url>,
    scope: Vec<String>,
    client_id_issued_at: i64,
}

impl From<oauth_service::ports::OAuthClientView> for OAuthClientAdminData {
    fn from(client: oauth_service::ports::OAuthClientView) -> Self {
        let mut redirect_uris = client.redirect_uris.into_iter().collect::<Vec<_>>();
        redirect_uris.sort_by(|left, right| left.as_str().cmp(right.as_str()));

        Self {
            client_id: client.client_id,
            client_name: client.name.into(),
            tos_uri: client.tos_uri,
            policy_uri: client.policy_uri,
            client_uri: client.client_uri,
            logo_uri: client.logo_uri,
            redirect_uris,
            scope: super::scope_strings(client.scopes),
            client_id_issued_at: client.created.unix_timestamp(),
        }
    }
}

pub async fn list_clients(
    State(state): State<OAuthState>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (context, _) = match protected_context(state.authenticator.as_ref(), &headers).await {
        Ok(value) => value,
        Err(response) => return no_store(*response),
    };
    let request = match parse_list_oauth_clients_query(raw_query.as_deref()) {
        Ok(request) => request,
        Err(error) => return no_store(error.into_response()),
    };

    match state.list_clients.execute(&context, request).await {
        Ok(result) => match response_from_result(result) {
            Ok(data) => no_store(Json(data).into_response()),
            Err(error) => no_store(error.into_response()),
        },
        Err(error) => no_store(ApiError::from(error).into_response()),
    }
}

fn parse_list_oauth_clients_query(
    raw_query: Option<&str>,
) -> Result<ListOAuthClientsRequest, ApiError> {
    let query: ListOAuthClientsQuery = serde_qs::Config::new()
        .use_form_encoding(true)
        .deserialize_str(raw_query.unwrap_or_default())
        .map_err(|error| bad_query("query", error))?;
    let client_id = query
        .client_id
        .map(|value| parse_query_object_id(&value, "clientId", "OAuthClient"))
        .transpose()?;
    let name_query = query
        .name
        .map(|value| {
            domain_primitives::query::text_query::TextQuery::<0>::try_from(value)
                .map_err(|error| bad_query("name", error))
        })
        .transpose()?;
    let size = query
        .size
        .map(|value| {
            value
                .parse::<u64>()
                .map(|size| size.clamp(1, MAX_PAGE_SIZE))
                .map_err(|error| bad_query("size", error))
        })
        .transpose()?;
    let search_after = query
        .search_after
        .as_deref()
        .map(parse_search_after)
        .transpose()?;

    Ok(ListOAuthClientsRequest {
        search: OAuthClientSearch {
            client_id,
            name_query,
        },
        cursor: if size.is_some() || search_after.is_some() {
            Some(Cursor {
                size: size.unwrap_or_else(|| Cursor::<OAuthClientSearchCursor>::default().size),
                search_after,
            })
        } else {
            None
        },
    })
}

fn parse_search_after(value: &str) -> Result<OAuthClientSearchCursor, ApiError> {
    let value: Value = serde_json::from_str(value).map_err(|error| {
        bad_query(
            "searchAfter",
            format!(
                "searchAfter must be a JSON array containing timestamp and OAuthClient ID: {error}"
            ),
        )
    })?;
    let Value::Array(values) = value else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must contain an RFC3339 timestamp and OAuthClient ID.",
        ));
    };
    let [Value::String(position), Value::String(client_id)] = values.as_slice() else {
        return Err(bad_query(
            "searchAfter",
            "searchAfter must contain an RFC3339 timestamp and OAuthClient ID.",
        ));
    };
    let position = OffsetDateTime::parse(position, &Rfc3339)
        .map_err(|error| bad_query("searchAfter", error))?;
    let client_id = parse_query_object_id(client_id, "searchAfter", "OAuthClient")?;

    Ok(OAuthClientSearchCursor {
        position,
        client_id,
    })
}

fn response_from_result(
    result: ListOAuthClientsResult,
) -> Result<JsonCursoredData<OAuthClientAdminData>, ApiError> {
    let CursoredResult {
        items,
        cursor,
        total,
    } = result;
    let search_after = cursor
        .search_after
        .map(serialize_search_after)
        .transpose()?;

    Ok(JsonCursoredData {
        items: items.into_iter().map(OAuthClientAdminData::from).collect(),
        size: cursor.size,
        search_after,
        total,
    })
}

fn serialize_search_after(cursor: OAuthClientSearchCursor) -> Result<Value, ApiError> {
    let position = cursor
        .position
        .format(&Rfc3339)
        .map_err(|_| ApiError::internal_server_error(OAUTH_INTERNAL_ERROR))?;
    Ok(json!([position, cursor.client_id]))
}

fn bad_query(field: &'static str, detail: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(BAD_QUERY_PARAMETER_VALUE)
        .with_query_field(field)
        .with_detail(detail.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn should_map_oauth_client_query_to_bounded_request() -> Result<(), ApiError> {
        let client_id = OAuthClientId::new();
        let cursor = OAuthClientSearchCursor {
            position: datetime!(2026-09-04 12:00 UTC),
            client_id,
        };
        let raw_cursor = serde_json::to_string(&json!([
            cursor
                .position
                .format(&Rfc3339)
                .map_err(|_| { ApiError::internal_server_error(OAUTH_INTERNAL_ERROR) })?,
            client_id.to_string()
        ]))
        .map_err(|error| {
            ApiError::internal_server_error(OAUTH_INTERNAL_ERROR).with_detail(error.to_string())
        })?;
        let request = parse_list_oauth_clients_query(Some(&format!(
            "clientId={client_id}&name=Dashboard&size=200&searchAfter={raw_cursor}"
        )))?;

        assert_eq!(Some(client_id), request.search.client_id);
        assert_eq!(Some("Dashboard"), request.search.name_query.as_deref());
        assert_eq!(
            Some(Cursor {
                size: 100,
                search_after: Some(cursor),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_reject_invalid_oauth_client_query_values() {
        for query in [
            "clientId=not-an-object-id",
            "size=not-a-number",
            "searchAfter=not-json",
            "searchAfter=%5B%22not-a-timestamp%22%2C%22not-an-object-id%22%5D",
        ] {
            assert!(
                parse_list_oauth_clients_query(Some(query)).is_err(),
                "{query}"
            );
        }
    }

    #[rstest::rstest]
    #[case("{}")]
    #[case("[]")]
    #[case(concat!("[", "\"timestamp-only\"", "]"))]
    #[case(concat!("[", "\"timestamp\",\"uuid\",\"extra\"", "]"))]
    #[case(concat!("[123,", "\"uuid\"", "]"))]
    #[case(concat!("[", "\"timestamp\",123]"))]
    #[tokio::test]
    async fn should_reject_search_after_when_json_value_is_not_a_two_string_array(
        #[case] value: &str,
    ) {
        let error = parse_search_after(value).expect_err("invalid cursor shape must be rejected");

        assert_eq!(BAD_QUERY_PARAMETER_VALUE, error.code());
        let body = axum::body::to_bytes(error.into_response().into_body(), usize::MAX)
            .await
            .unwrap_or_else(|error| panic!("problem response must serialize: {error}"));
        let body: Value = serde_json::from_slice(&body)
            .unwrap_or_else(|error| panic!("problem response must be JSON: {error}"));
        assert_eq!("searchAfter", body["source"]["field"]);
        assert_eq!("QUERY", body["source"]["type"]);
    }

    #[test]
    fn should_default_page_size_when_only_search_after_is_provided() -> Result<(), ApiError> {
        let client_id = OAuthClientId::new();
        let cursor = OAuthClientSearchCursor {
            position: time::macros::datetime!(2026-09-04 12:00 UTC),
            client_id,
        };
        let raw_cursor = serde_json::json!([
            cursor
                .position
                .format(&Rfc3339)
                .map_err(|_| ApiError::internal_server_error(OAUTH_INTERNAL_ERROR))?,
            client_id.to_string()
        ])
        .to_string();
        let request = parse_list_oauth_clients_query(Some(&format!("searchAfter={raw_cursor}")))?;

        assert_eq!(
            Some(Cursor {
                size: Cursor::<OAuthClientSearchCursor>::default().size,
                search_after: Some(cursor),
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_clamp_page_size_to_one_when_zero() -> Result<(), ApiError> {
        let request = parse_list_oauth_clients_query(Some("size=0"))?;

        assert_eq!(
            Some(Cursor {
                size: 1,
                search_after: None,
            }),
            request.cursor
        );
        Ok(())
    }

    #[test]
    fn should_return_internal_error_when_search_after_timestamp_cannot_be_serialized_as_rfc3339() {
        let error = serialize_search_after(OAuthClientSearchCursor {
            position: time::Date::MIN.midnight().assume_utc(),
            client_id: OAuthClientId::new(),
        })
        .expect_err("the minimum timestamp is outside RFC3339's representable range");

        assert_eq!(OAUTH_INTERNAL_ERROR, error.code());
    }

    #[test]
    fn should_omit_client_secret_from_admin_list_item() {
        let client = oauth_service::ports::OAuthClientView {
            client_id: OAuthClientId::new(),
            name: oauth_core::client::OAuthClientName::from("Dashboard"),
            redirect_uris: std::collections::HashSet::from([Url::parse(
                "https://client.example/callback",
            )
            .unwrap_or_else(|error| panic!("valid test URL: {error}"))]),
            tos_uri: Url::parse("https://client.example/tos")
                .unwrap_or_else(|error| panic!("valid test URL: {error}")),
            policy_uri: Url::parse("https://client.example/policy")
                .unwrap_or_else(|error| panic!("valid test URL: {error}")),
            client_uri: Url::parse("https://client.example")
                .unwrap_or_else(|error| panic!("valid test URL: {error}")),
            logo_uri: Url::parse("https://client.example/logo.png")
                .unwrap_or_else(|error| panic!("valid test URL: {error}")),
            scopes: std::collections::HashSet::new(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        };
        let value = serde_json::to_value(OAuthClientAdminData::from(client))
            .unwrap_or_else(|error| panic!("admin item serializes: {error}"));

        assert!(value.get("client_secret").is_none());
    }
}

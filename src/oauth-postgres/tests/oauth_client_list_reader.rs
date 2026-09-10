use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::scope::Scope;
use domain_primitives::query::text_query::TextQuery;
use oauth_core::{
    OAuthClientSearch,
    client::{OAuthClient, OAuthClientName, OAuthRedirectUris, RehydratedOAuthClientState},
};
use oauth_postgres::{SqlxOAuthClientListReader, SqlxOAuthClientRepositoryFactory};
use oauth_service::ports::{
    OAuthClientListReader, OAuthClientReadError, OAuthClientRepository,
    OAuthClientRepositoryFactory,
};
use oauth_service::use_cases::ListOAuthClientsRequest;
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::collections::HashSet;
use std::error::Error as StdError;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_search_oauth_clients_by_literal_name_when_query_contains_like_wildcards() {
    let result: Result<(), Box<dyn StdError>> = async {
        let pool = get_postgres_client().await;
        let matching = oauth_client(r"literal 100%_ready\ marker")?;
        let percent_wildcard = oauth_client(r"literal 100X_ready\ marker")?;
        let underscore_wildcard = oauth_client(r"literal 100%Xready\ marker")?;
        let backslash_wildcard = oauth_client(r"literal 100%_readyX marker")?;
        for client in [
            &matching,
            &percent_wildcard,
            &underscore_wildcard,
            &backslash_wildcard,
        ] {
            insert_client(&pool, client).await?;
        }

        let reader = SqlxOAuthClientListReader::new(pool);
        let result = reader
            .search(&ListOAuthClientsRequest {
                search: OAuthClientSearch {
                    name_query: Some(TextQuery::try_from(r"100%_ready\")?),
                    ..Default::default()
                },
                cursor: None,
            })
            .await?;

        assert_eq!(
            vec![matching.client_id()],
            result
                .items
                .iter()
                .map(|item| item.client_id)
                .collect::<Vec<_>>()
        );
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "literal wildcard search failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_clamp_oauth_client_search_page_size_to_one_and_one_hundred() {
    let result: Result<(), Box<dyn StdError>> = async {
        let pool = get_postgres_client().await;
        for name in ["clamp target one", "clamp target two"] {
            let client = oauth_client(name)?;
            insert_client(&pool, &client).await?;
        }
        let reader = SqlxOAuthClientListReader::new(pool);
        let search = OAuthClientSearch {
            name_query: Some(TextQuery::try_from("clamp target")?),
            ..Default::default()
        };

        let lower = reader
            .search(&ListOAuthClientsRequest {
                search: search.clone(),
                cursor: Some(Cursor {
                    size: 0,
                    search_after: None,
                }),
            })
            .await?;
        assert_eq!(1, lower.cursor.size);
        assert_eq!(1, lower.items.len());
        assert!(lower.cursor.search_after.is_some());

        let upper = reader
            .search(&ListOAuthClientsRequest {
                search,
                cursor: Some(Cursor {
                    size: 101,
                    search_after: None,
                }),
            })
            .await?;
        assert_eq!(100, upper.cursor.size);
        assert_eq!(2, upper.items.len());
        assert!(upper.cursor.search_after.is_none());
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "page-size bounds failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_return_no_search_after_when_page_is_exactly_full_without_an_extra_row() {
    let result: Result<(), Box<dyn StdError>> = async {
        let pool = get_postgres_client().await;
        for name in ["exact page one", "exact page two"] {
            let client = oauth_client(name)?;
            insert_client(&pool, &client).await?;
        }
        let result = SqlxOAuthClientListReader::new(pool)
            .search(&ListOAuthClientsRequest {
                search: OAuthClientSearch {
                    name_query: Some(TextQuery::try_from("exact page")?),
                    ..Default::default()
                },
                cursor: Some(Cursor {
                    size: 2,
                    search_after: None,
                }),
            })
            .await?;

        assert_eq!(2, result.items.len());
        assert!(result.cursor.search_after.is_none());
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "exact-page cursor test failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_order_equal_created_oauth_clients_by_client_id() {
    let result: Result<(), Box<dyn StdError>> = async {
        let pool = get_postgres_client().await;
        let clients = [
            oauth_client("equal-created alpha")?,
            oauth_client("equal-created beta")?,
            oauth_client("equal-created gamma")?,
        ];
        for client in &clients {
            insert_client(&pool, client).await?;
        }
        let created = OffsetDateTime::from_unix_timestamp(1_700_000_000)?;
        for client in &clients {
            sqlx::query("UPDATE oauth_clients SET created = $1, updated = $1 WHERE client_id = $2")
                .bind(created)
                .bind(client.client_id().into_uuid())
                .execute(&pool)
                .await?;
        }

        let mut expected = clients
            .iter()
            .map(OAuthClient::client_id)
            .collect::<Vec<_>>();
        expected.sort_by_key(|client_id| client_id.to_string());
        let reader = SqlxOAuthClientListReader::new(pool);
        let first = reader
            .search(&ListOAuthClientsRequest {
                search: OAuthClientSearch {
                    name_query: Some(TextQuery::try_from("equal-created")?),
                    ..Default::default()
                },
                cursor: Some(Cursor {
                    size: 2,
                    search_after: None,
                }),
            })
            .await?;
        let second = reader
            .search(&ListOAuthClientsRequest {
                search: OAuthClientSearch {
                    name_query: Some(TextQuery::try_from("equal-created")?),
                    ..Default::default()
                },
                cursor: Some(Cursor {
                    size: 2,
                    search_after: first.cursor.search_after,
                }),
            })
            .await?;
        let actual = first
            .items
            .into_iter()
            .chain(second.items)
            .map(|item| item.client_id)
            .collect::<Vec<_>>();

        assert_eq!(expected, actual);
        assert!(second.cursor.search_after.is_none());
        Ok(())
    }
    .await;

    assert!(result.is_ok(), "equal-created ordering failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_filter_oauth_clients_by_client_id_and_name_intersection() {
    let result: Result<(), Box<dyn StdError>> = async {
        let pool = get_postgres_client().await;
        let target = oauth_client("intersection target")?;
        let other = oauth_client("intersection other")?;
        insert_client(&pool, &target).await?;
        insert_client(&pool, &other).await?;
        let reader = SqlxOAuthClientListReader::new(pool);

        let no_match = reader
            .search(&ListOAuthClientsRequest {
                search: OAuthClientSearch {
                    client_id: Some(target.client_id()),
                    name_query: Some(TextQuery::try_from("other")?),
                },
                cursor: None,
            })
            .await?;
        assert!(no_match.items.is_empty());

        let match_result = reader
            .search(&ListOAuthClientsRequest {
                search: OAuthClientSearch {
                    client_id: Some(target.client_id()),
                    name_query: Some(TextQuery::try_from("target")?),
                },
                cursor: None,
            })
            .await?;
        assert_eq!(
            vec![target.client_id()],
            match_result
                .items
                .iter()
                .map(|item| item.client_id)
                .collect::<Vec<_>>()
        );
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth client filter intersection failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_invalid_persisted_oauth_client_row_when_listing() {
    let result: Result<(), Box<dyn StdError>> = async {
        let pool = get_postgres_client().await;
        let client = oauth_client("corrupt list row")?;
        insert_client(&pool, &client).await?;
        sqlx::query("UPDATE oauth_clients SET redirect_uris = $1 WHERE client_id = $2")
            .bind(vec!["http://client.example/callback"])
            .bind(client.client_id().into_uuid())
            .execute(&pool)
            .await?;

        let error = SqlxOAuthClientListReader::new(pool)
            .search(&ListOAuthClientsRequest {
                search: OAuthClientSearch {
                    client_id: Some(client.client_id()),
                    ..Default::default()
                },
                cursor: None,
            })
            .await
            .expect_err("corrupt row must not become an OAuth client view");
        assert!(matches!(
            error,
            OAuthClientReadError::InvalidPersistedState { .. }
        ));
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "invalid persisted list row failed: {result:?}"
    );
}

fn oauth_client(name: &str) -> Result<OAuthClient, Box<dyn StdError>> {
    Ok(OAuthClient::create(RehydratedOAuthClientState {
        client_id: credential_core::oauth_client_id::OAuthClientId::new(),
        hashed_client_secret: user_core::access_token::HashedRawOAuthClientSecret::new(
            "test-short-token".to_owned(),
            "test-long-token-hash".to_owned(),
        ),
        name: OAuthClientName::from(name),
        redirect_uris: OAuthRedirectUris::try_from(HashSet::from([Url::parse(
            "https://client.example/callback",
        )?]))?,
        tos_uri: Url::parse("https://client.example/tos")?,
        policy_uri: Url::parse("https://client.example/policy")?,
        client_uri: Url::parse("https://client.example")?,
        logo_uri: Url::parse("https://client.example/logo.svg")?,
        scopes: HashSet::from([Scope::AccessTokensRead]),
    }))
}

async fn insert_client(pool: &PgPool, client: &OAuthClient) -> Result<(), Box<dyn StdError>> {
    let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
    SqlxOAuthClientRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(client)
        .await?;
    transaction.commit().await?;
    Ok(())
}

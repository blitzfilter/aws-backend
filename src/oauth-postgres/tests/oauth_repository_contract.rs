use application::pagination::Cursor;
use application::transaction::{Transaction, UnitOfWork};
use credential_core::{oauth_client_id::OAuthClientId, scope::Scope};
use domain_primitives::query::text_query::TextQuery;
use oauth_core::{
    OAuthClientSearch,
    authorization_code::{
        AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
        RehydratedAuthorizationCodeState,
    },
    client::{OAuthClient, OAuthClientName, OAuthRedirectUris, RehydratedOAuthClientState},
    third_party_exchange_code::{
        RehydratedThirdPartyExchangeCodeGrantState, ThirdPartyExchangeCode,
        ThirdPartyExchangeCodeGrant,
    },
};
use oauth_postgres::{
    SqlxAuthorizationCodeRepositoryFactory, SqlxOAuthClientAuthenticationReader,
    SqlxOAuthClientListReader, SqlxOAuthClientRepositoryFactory,
    SqlxThirdPartyExchangeCodeRepositoryFactory,
};
use oauth_service::ports::{
    AuthorizationCodeRepository, AuthorizationCodeRepositoryFactory,
    OAuthClientAuthenticationReader, OAuthClientListReader, OAuthClientRepository,
    OAuthClientRepositoryError, OAuthClientRepositoryFactory, OAuthCodeRepositoryError,
    ThirdPartyExchangeCodeRepository, ThirdPartyExchangeCodeRepositoryFactory,
};
use oauth_service::use_cases::ListOAuthClientsRequest;
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use std::collections::HashSet;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use url::Url;
use user_core::{
    access_token::{HashedRawOAuthClientSecret, RawAccessToken},
    user_id::UserId,
};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const DUMMY_CLIENT_SECRET_SHORT: &str = "dummy-client-secret-short";
const DUMMY_CLIENT_SECRET_HASH: &str = "dummy-client-secret-long-token-hash";

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_consume_authorization_code_once_and_allow_one_concurrent_consumer() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let client_id = seed_oauth_client(&pool, "authorization-code-client").await?;
        let user_id = seed_user(&pool).await?;

        let code = authorization_code(client_id, user_id)?;
        let code_value = code.code();
        insert_authorization_code(pool.clone(), code.clone()).await?;

        let first = consume_authorization_code(pool.clone(), code_value).await?;
        assert!(
            first.is_some(),
            "the first authorization-code consumer must receive the code"
        );
        assert!(
            consume_authorization_code(pool.clone(), code_value)
                .await?
                .is_none(),
            "a consumed authorization code must not be returned again"
        );

        let concurrent_code = authorization_code(client_id, user_id)?;
        let code_value = concurrent_code.code();
        insert_authorization_code(pool.clone(), concurrent_code).await?;
        let (first, second) = tokio::join!(
            consume_authorization_code(pool.clone(), code_value),
            consume_authorization_code(pool.clone(), code_value),
        );
        let results = [first?, second?];
        assert_eq!(
            1,
            results.iter().filter(|result| result.is_some()).count(),
            "exactly one concurrent authorization-code consumer must receive the code"
        );
        assert_eq!(
            1,
            results.iter().filter(|result| result.is_none()).count(),
            "one concurrent authorization-code consumer must observe an absent code"
        );

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "authorization-code repository integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_consume_third_party_exchange_code_once_and_allow_one_concurrent_consumer() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await?;
        let grant = third_party_exchange_code_grant();
        seed_access_token_for_grant(&pool, &grant, user_id).await?;
        let code_value = grant.code();
        insert_third_party_exchange_code(pool.clone(), grant.clone()).await?;

        let first = consume_third_party_exchange_code(pool.clone(), code_value).await?;
        assert!(
            first.is_some(),
            "the first third-party exchange-code consumer must receive the grant"
        );
        assert!(
            consume_third_party_exchange_code(pool.clone(), code_value)
                .await?
                .is_none(),
            "a consumed third-party exchange code must not be returned again"
        );

        let concurrent_grant = third_party_exchange_code_grant();
        seed_access_token_for_grant(&pool, &concurrent_grant, user_id).await?;
        let code_value = concurrent_grant.code();
        insert_third_party_exchange_code(pool.clone(), concurrent_grant).await?;
        let (first, second) = tokio::join!(
            consume_third_party_exchange_code(pool.clone(), code_value),
            consume_third_party_exchange_code(pool.clone(), code_value),
        );
        let results = [first?, second?];
        assert_eq!(
            1,
            results.iter().filter(|result| result.is_some()).count(),
            "exactly one concurrent third-party exchange-code consumer must receive the grant"
        );
        assert_eq!(
            1,
            results.iter().filter(|result| result.is_none()).count(),
            "one concurrent third-party exchange-code consumer must observe an absent grant"
        );

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "third-party exchange-code repository integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_read_oauth_client_secret_hash_only_for_authentication() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let client = oauth_client(OAuthClientId::new(), "authentication-client")?;
        insert_oauth_client(pool.clone(), &client).await?;
        let reader = SqlxOAuthClientAuthenticationReader::new(pool);

        let authentication = reader
            .find_by_id(&client.client_id())
            .await?
            .ok_or_else(|| std::io::Error::other("missing OAuth client authentication material"))?;
        let missing = reader.find_by_id(&OAuthClientId::new()).await?;

        assert_eq!(
            client.hashed_client_secret(),
            &authentication.hashed_client_secret
        );
        assert!(missing.is_none());
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth client authentication reader integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_persist_oauth_client_secret_hash_and_reject_stale_update() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let client = oauth_client(OAuthClientId::new(), "versioned-client")?;
        let inserted = insert_oauth_client(pool.clone(), &client).await?;
        assert!(inserted.created > OffsetDateTime::UNIX_EPOCH);
        assert_eq!(inserted.created, inserted.updated);

        let stored_secret = client_secret_columns(&pool, client.client_id()).await?;
        assert_eq!(
            (
                DUMMY_CLIENT_SECRET_SHORT.to_owned(),
                DUMMY_CLIENT_SECRET_HASH.to_owned()
            ),
            stored_secret
        );

        let mut updated_client = inserted.value.clone();
        updated_client.change_name(OAuthClientName::from("updated-client"));
        let updated = update_oauth_client(pool.clone(), &updated_client, inserted.version).await?;
        assert_ne!(inserted.version, updated.version);
        assert_eq!(inserted.created, updated.created);
        assert!(updated.updated >= inserted.updated);
        assert_eq!(
            &OAuthClientName::from("updated-client"),
            updated.value.name()
        );

        let stored_secret_after_update = client_secret_columns(&pool, client.client_id()).await?;
        assert_eq!(stored_secret, stored_secret_after_update);

        let mut stale_client = inserted.value;
        stale_client.change_name(OAuthClientName::from("stale-client"));
        let stale_update = update_oauth_client(pool, &stale_client, inserted.version).await;
        assert!(matches!(
            stale_update,
            Err(OAuthClientRepositoryError::ConcurrencyConflict)
        ));

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth client repository integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_classify_duplicate_oauth_writes_as_conflicts() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;

        let client = oauth_client(OAuthClientId::new(), "duplicate-client")?;
        insert_oauth_client(pool.clone(), &client).await?;
        let duplicate = insert_oauth_client(pool.clone(), &client).await;
        assert!(matches!(
            duplicate,
            Err(OAuthClientRepositoryError::Conflict { .. })
        ));

        let user_id = seed_user(&pool).await?;
        let code = authorization_code(client.client_id(), user_id)?;
        insert_authorization_code(pool.clone(), code.clone()).await?;
        let duplicate = insert_authorization_code(pool.clone(), code).await;
        let duplicate_error = match duplicate {
            Ok(()) => panic!("duplicate authorization code insert must fail"),
            Err(error) => error,
        };
        assert!(matches!(
            duplicate_error.downcast_ref::<OAuthCodeRepositoryError>(),
            Some(OAuthCodeRepositoryError::Conflict { .. })
        ));

        let grant = third_party_exchange_code_grant();
        seed_access_token_for_grant(&pool, &grant, user_id).await?;
        insert_third_party_exchange_code(pool.clone(), grant.clone()).await?;
        let duplicate = insert_third_party_exchange_code(pool, grant).await;
        let duplicate_error = match duplicate {
            Ok(()) => panic!("duplicate third-party code insert must fail"),
            Err(error) => error,
        };
        assert!(matches!(
            duplicate_error.downcast_ref::<OAuthCodeRepositoryError>(),
            Some(OAuthCodeRepositoryError::Conflict { .. })
        ));

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "duplicate OAuth writes must classify as conflicts: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_invalid_persisted_redirect_uris() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        for redirect_uri in [
            "http://client.example/callback",
            "https://client.example/callback#fragment",
        ] {
            let client = oauth_client(OAuthClientId::new(), "corrupt-client")?;
            insert_oauth_client(pool.clone(), &client).await?;
            sqlx::query("UPDATE oauth_clients SET redirect_uris = $1 WHERE client_id = $2")
                .bind(vec![redirect_uri.to_owned()])
                .bind(Uuid::parse_str(&client.client_id().to_string())?)
                .execute(&pool)
                .await?;

            let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
            let loaded = SqlxOAuthClientRepositoryFactory::new()
                .in_transaction(&mut transaction)
                .find_by_id(client.client_id())
                .await;
            assert!(matches!(
                loaded,
                Err(OAuthClientRepositoryError::InvalidPersistedState { .. })
            ));
        }

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "invalid persisted redirect URIs must be rejected: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_search_oauth_clients_with_bounded_deterministic_cursor() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let first = oauth_client(OAuthClientId::new(), "List Reader Alpha")?;
        let second = oauth_client(OAuthClientId::new(), "List Reader Beta")?;
        let third = oauth_client(OAuthClientId::new(), "List Reader Gamma")?;
        let first_inserted = insert_oauth_client(pool.clone(), &first).await?;
        let second_inserted = insert_oauth_client(pool.clone(), &second).await?;
        let third_inserted = insert_oauth_client(pool.clone(), &third).await?;

        let mut expected = [
            (first_inserted.created, first.client_id()),
            (second_inserted.created, second.client_id()),
            (third_inserted.created, third.client_id()),
        ];
        expected.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
        });
        let search = OAuthClientSearch {
            name_query: Some(TextQuery::try_from("list reader")?),
            ..Default::default()
        };
        let reader = SqlxOAuthClientListReader::new(pool);
        let first_page = reader
            .search(&ListOAuthClientsRequest {
                search: search.clone(),
                cursor: Some(Cursor {
                    size: 2,
                    search_after: None,
                }),
            })
            .await?;
        let expected_ids = expected
            .iter()
            .map(|(_, client_id)| *client_id)
            .collect::<Vec<_>>();
        assert_eq!(
            expected_ids[..2].to_vec(),
            first_page
                .items
                .iter()
                .map(|item| item.client_id)
                .collect::<Vec<_>>()
        );
        assert!(first_page.cursor.search_after.is_some());

        let second_page = reader
            .search(&ListOAuthClientsRequest {
                search,
                cursor: Some(Cursor {
                    size: 2,
                    search_after: first_page.cursor.search_after,
                }),
            })
            .await?;
        assert_eq!(
            vec![expected[2].1],
            second_page
                .items
                .iter()
                .map(|item| item.client_id)
                .collect::<Vec<_>>()
        );
        assert!(second_page.cursor.search_after.is_none());

        let exact = reader
            .search(&ListOAuthClientsRequest {
                search: OAuthClientSearch {
                    client_id: Some(second.client_id()),
                    ..Default::default()
                },
                cursor: None,
            })
            .await?;
        assert_eq!(
            vec![second.client_id()],
            exact
                .items
                .iter()
                .map(|item| item.client_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(2, first_page.cursor.size);
        assert_eq!(21, exact.cursor.size);
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth client list reader integration test failed: {result:?}"
    );
}

async fn seed_oauth_client(
    pool: &PgPool,
    name: &str,
) -> Result<OAuthClientId, Box<dyn std::error::Error>> {
    let client = oauth_client(OAuthClientId::new(), name)?;
    insert_oauth_client(pool.clone(), &client).await?;
    Ok(client.client_id())
}

async fn seed_user(pool: &PgPool) -> Result<UserId, Box<dyn std::error::Error>> {
    let user_id = UserId::from(Uuid::now_v7());
    let email = format!("dummy-oauth-code-{}@example.test", Uuid::now_v7());
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(Uuid::parse_str(&user_id.to_string())?)
        .bind(email)
        .execute(pool)
        .await?;
    Ok(user_id)
}

fn oauth_client(client_id: OAuthClientId, name: &str) -> Result<OAuthClient, url::ParseError> {
    let redirect_uri = Url::parse("https://dummy.example.test/oauth/callback")?;
    let redirect_uris = match OAuthRedirectUris::try_from(HashSet::from([redirect_uri])) {
        Ok(value) => value,
        Err(error) => panic!("test redirect URI must be valid: {error}"),
    };
    Ok(OAuthClient::create(RehydratedOAuthClientState {
        client_id,
        hashed_client_secret: HashedRawOAuthClientSecret::new(
            DUMMY_CLIENT_SECRET_SHORT.to_owned(),
            DUMMY_CLIENT_SECRET_HASH.to_owned(),
        ),
        name: OAuthClientName::from(name),
        redirect_uris,
        tos_uri: Url::parse("https://dummy.example.test/tos")?,
        policy_uri: Url::parse("https://dummy.example.test/policy")?,
        client_uri: Url::parse("https://dummy.example.test")?,
        logo_uri: Url::parse("https://dummy.example.test/logo.svg")?,
        scopes: HashSet::from([Scope::ProductListingsWrite]),
    }))
}

fn authorization_code(
    client_id: OAuthClientId,
    user_id: UserId,
) -> Result<AuthorizationCode, url::ParseError> {
    let now = OffsetDateTime::now_utc();
    Ok(AuthorizationCode::create(
        RehydratedAuthorizationCodeState {
            code: OAuthAuthorizationCode::from(Uuid::now_v7()),
            client_id,
            user_id,
            redirect_uri: Url::parse("https://dummy.example.test/oauth/callback")?,
            scopes: HashSet::from([Scope::ProductListingsWrite]),
            code_challenge: OAuthCodeChallenge::from("dummy-pkce-code-challenge"),
            code_challenge_method: CodeChallengeMethod::S256,
            expires: now + Duration::minutes(5),
        },
    ))
}

async fn seed_access_token_for_grant(
    pool: &PgPool,
    grant: &ThirdPartyExchangeCodeGrant,
    user_id: UserId,
) -> Result<(), Box<dyn std::error::Error>> {
    let hashed = user_core::access_token::HashedRawAccessToken::from(grant.access_token().clone());
    sqlx::query(
        "INSERT INTO access_tokens (access_token_id, user_id, token_short, token_hash, name, scopes, origin) \
         VALUES ($1, $2, $3, $4, $5, $6, 'USER')",
    )
    .bind(Uuid::parse_str(&grant.access_token_id().to_string())?)
    .bind(Uuid::parse_str(&user_id.to_string())?)
    .bind(hashed.short_token())
    .bind(hashed.long_token_hash())
    .bind("third-party exchange test token")
    .bind(vec!["product-listings:write"])
    .execute(pool)
    .await?;
    Ok(())
}

fn third_party_exchange_code_grant() -> ThirdPartyExchangeCodeGrant {
    let now = OffsetDateTime::now_utc();
    ThirdPartyExchangeCodeGrant::create(RehydratedThirdPartyExchangeCodeGrantState {
        code: ThirdPartyExchangeCode::from(Uuid::now_v7()),
        access_token_id: user_core::access_token::AccessTokenId::new(),
        access_token: RawAccessToken::new(),
        access_token_expires: Some(now + Duration::minutes(10)),
        scopes: HashSet::from([Scope::ProductListingsWrite]),
        expires: now + Duration::minutes(5),
    })
}

async fn insert_oauth_client(
    pool: PgPool,
    client: &OAuthClient,
) -> Result<oauth_service::ports::VersionedOAuthClient, OAuthClientRepositoryError> {
    let mut transaction = SqlxUnitOfWork::new(pool).begin().await.map_err(|error| {
        OAuthClientRepositoryError::Internal {
            source: Box::new(error),
        }
    })?;
    let inserted = SqlxOAuthClientRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(client)
        .await?;
    transaction
        .commit()
        .await
        .map_err(|error| OAuthClientRepositoryError::Internal {
            source: Box::new(error),
        })?;
    Ok(inserted)
}

async fn update_oauth_client(
    pool: PgPool,
    client: &OAuthClient,
    expected_version: oauth_service::ports::OAuthClientStorageVersion,
) -> Result<oauth_service::ports::VersionedOAuthClient, OAuthClientRepositoryError> {
    let mut transaction = SqlxUnitOfWork::new(pool).begin().await.map_err(|error| {
        OAuthClientRepositoryError::Internal {
            source: Box::new(error),
        }
    })?;
    let updated = SqlxOAuthClientRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .update(client, expected_version)
        .await?;
    transaction
        .commit()
        .await
        .map_err(|error| OAuthClientRepositoryError::Internal {
            source: Box::new(error),
        })?;
    Ok(updated)
}

async fn insert_authorization_code(
    pool: PgPool,
    code: AuthorizationCode,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool).begin().await?;
    SqlxAuthorizationCodeRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(code)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn consume_authorization_code(
    pool: PgPool,
    code: OAuthAuthorizationCode,
) -> Result<Option<AuthorizationCode>, Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool).begin().await?;
    let consumed = SqlxAuthorizationCodeRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .consume_by_code(&code)
        .await?;
    transaction.commit().await?;
    Ok(consumed)
}

async fn insert_third_party_exchange_code(
    pool: PgPool,
    grant: ThirdPartyExchangeCodeGrant,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool).begin().await?;
    SqlxThirdPartyExchangeCodeRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(grant)
        .await?;
    transaction.commit().await?;
    Ok(())
}

async fn consume_third_party_exchange_code(
    pool: PgPool,
    code: ThirdPartyExchangeCode,
) -> Result<Option<ThirdPartyExchangeCodeGrant>, Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool).begin().await?;
    let consumed = SqlxThirdPartyExchangeCodeRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .consume_by_code(&code)
        .await?;
    transaction.commit().await?;
    Ok(consumed)
}

async fn client_secret_columns(
    pool: &PgPool,
    client_id: OAuthClientId,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    sqlx::query_as(
        "SELECT client_secret_short_token, client_secret_long_token_hash \
         FROM oauth_clients WHERE client_id = $1",
    )
    .bind(Uuid::parse_str(&client_id.to_string())?)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

const USER_ID: Uuid = Uuid::from_u128(0x10000000000000000000000000000001);
const CLIENT_ID: Uuid = Uuid::from_u128(0x20000000000000000000000000000001);
const EXPIRED_ACCESS_TOKEN_ID: Uuid = Uuid::from_u128(0x30000000000000000000000000000001);
const FUTURE_ACCESS_TOKEN_ID: Uuid = Uuid::from_u128(0x30000000000000000000000000000002);
const NON_EXPIRING_ACCESS_TOKEN_ID: Uuid = Uuid::from_u128(0x30000000000000000000000000000003);
const EXPIRED_AUTHORIZATION_CODE: Uuid = Uuid::from_u128(0x40000000000000000000000000000001);
const FUTURE_AUTHORIZATION_CODE: Uuid = Uuid::from_u128(0x40000000000000000000000000000002);
const EXPIRED_THIRD_PARTY_EXCHANGE_CODE: Uuid = Uuid::from_u128(0x50000000000000000000000000000001);
const FUTURE_THIRD_PARTY_EXCHANGE_CODE: Uuid = Uuid::from_u128(0x50000000000000000000000000000002);

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_register_and_apply_pg_ttl_for_expiring_oauth_credentials() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;

        let registrations: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT schema_name, table_name, column_name \
             FROM ttl_summary() \
             ORDER BY schema_name, table_name, column_name",
        )
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            registrations,
            vec![
                (
                    String::from("public"),
                    String::from("access_tokens"),
                    String::from("expires_at"),
                ),
                (
                    String::from("public"),
                    String::from("oauth_authorization_codes"),
                    String::from("expires_at"),
                ),
                (
                    String::from("public"),
                    String::from("oauth_third_party_exchange_codes"),
                    String::from("expires_at"),
                ),
            ]
        );

        let expired_at = OffsetDateTime::now_utc() - Duration::hours(1);
        let future_at = OffsetDateTime::now_utc() + Duration::hours(1);
        seed_user_and_client(&pool).await?;
        seed_access_token(
            &pool,
            EXPIRED_ACCESS_TOKEN_ID,
            "dummy-access-token-expired",
            Some(expired_at),
        )
        .await?;
        seed_access_token(
            &pool,
            FUTURE_ACCESS_TOKEN_ID,
            "dummy-access-token-future",
            Some(future_at),
        )
        .await?;
        seed_access_token(
            &pool,
            NON_EXPIRING_ACCESS_TOKEN_ID,
            "dummy-access-token-non-expiring",
            None,
        )
        .await?;
        seed_authorization_code(
            &pool,
            EXPIRED_AUTHORIZATION_CODE,
            "dummy-code-expired",
            expired_at,
        )
        .await?;
        seed_authorization_code(
            &pool,
            FUTURE_AUTHORIZATION_CODE,
            "dummy-code-future",
            future_at,
        )
        .await?;
        seed_third_party_exchange_code(
            &pool,
            EXPIRED_THIRD_PARTY_EXCHANGE_CODE,
            EXPIRED_ACCESS_TOKEN_ID,
            "dummy-third-party-code-expired",
            expired_at,
        )
        .await?;
        seed_third_party_exchange_code(
            &pool,
            FUTURE_THIRD_PARTY_EXCHANGE_CODE,
            FUTURE_ACCESS_TOKEN_ID,
            "dummy-third-party-code-future",
            future_at,
        )
        .await?;

        sqlx::query("SELECT ttl_runner()").execute(&pool).await?;

        assert!(!access_token_exists(&pool, EXPIRED_ACCESS_TOKEN_ID).await?);
        assert!(!authorization_code_exists(&pool, EXPIRED_AUTHORIZATION_CODE).await?);
        assert!(!third_party_exchange_code_exists(&pool, EXPIRED_THIRD_PARTY_EXCHANGE_CODE).await?);
        assert!(access_token_exists(&pool, FUTURE_ACCESS_TOKEN_ID).await?);
        assert!(authorization_code_exists(&pool, FUTURE_AUTHORIZATION_CODE).await?);
        assert!(third_party_exchange_code_exists(&pool, FUTURE_THIRD_PARTY_EXCHANGE_CODE).await?);
        assert!(access_token_exists(&pool, NON_EXPIRING_ACCESS_TOKEN_ID).await?);

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth pg-ttl migration integration test failed: {result:?}"
    );
}

async fn seed_user_and_client(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) \
         VALUES ($1, 'dummy-oauth-ttl@example.test', 'FREE', 'USER')",
    )
    .bind(USER_ID)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO oauth_clients ( \
             client_id, client_secret_short_token, client_secret_long_token_hash, name, \
             redirect_uris, tos_uri, policy_uri, client_uri, logo_uri \
         ) VALUES ( \
             $1, 'dummy-client-secret-short', 'dummy-client-secret-hash', 'dummy-client', \
             ARRAY['https://example.test/oauth/callback'], 'https://example.test/tos', \
             'https://example.test/policy', 'https://example.test', \
             'https://example.test/logo.svg' \
         )",
    )
    .bind(CLIENT_ID)
    .execute(pool)
    .await?;

    Ok(())
}

async fn seed_access_token(
    pool: &PgPool,
    access_token_id: Uuid,
    token_suffix: &str,
    expires_at: Option<OffsetDateTime>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO access_tokens ( \
             access_token_id, user_id, token_short, token_hash, name, origin, oauth_client_id, expires_at \
         ) VALUES ($1, $2, $3, $4, $5, 'OAUTH', $6, $7)",
    )
    .bind(access_token_id)
    .bind(USER_ID)
    .bind(format!("{token_suffix}-short"))
    .bind(format!("{token_suffix}-hash"))
    .bind(token_suffix)
    .bind(CLIENT_ID)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

async fn seed_authorization_code(
    pool: &PgPool,
    authorization_code: Uuid,
    code_challenge: &str,
    expires_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_authorization_codes ( \
             authorization_code, client_id, user_id, redirect_uri, code_challenge, \
             code_challenge_method, expires_at \
         ) VALUES ($1, $2, $3, 'https://example.test/oauth/callback', $4, 'S256', $5)",
    )
    .bind(authorization_code)
    .bind(CLIENT_ID)
    .bind(USER_ID)
    .bind(code_challenge)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

async fn seed_third_party_exchange_code(
    pool: &PgPool,
    third_party_exchange_code: Uuid,
    access_token_id: Uuid,
    access_token: &str,
    expires_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_third_party_exchange_codes ( \
             third_party_exchange_code, access_token_id, access_token, expires_at \
         ) VALUES ($1, $2, $3, $4)",
    )
    .bind(third_party_exchange_code)
    .bind(access_token_id)
    .bind(access_token)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

async fn access_token_exists(pool: &PgPool, access_token_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM access_tokens WHERE access_token_id = $1)")
        .bind(access_token_id)
        .fetch_one(pool)
        .await
}

async fn authorization_code_exists(
    pool: &PgPool,
    authorization_code: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS( \
         SELECT 1 FROM oauth_authorization_codes WHERE authorization_code = $1)",
    )
    .bind(authorization_code)
    .fetch_one(pool)
    .await
}

async fn third_party_exchange_code_exists(
    pool: &PgPool,
    third_party_exchange_code: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS( \
         SELECT 1 FROM oauth_third_party_exchange_codes WHERE third_party_exchange_code = $1)",
    )
    .bind(third_party_exchange_code)
    .fetch_one(pool)
    .await
}

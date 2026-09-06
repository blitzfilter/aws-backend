use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct OAuthClientRow {
    pub(crate) client_id: Uuid,
    pub(crate) secret_short: String,
    pub(crate) secret_hash: String,
    pub(crate) name: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) tos_uri: String,
    pub(crate) policy_uri: String,
    pub(crate) client_uri: String,
    pub(crate) logo_uri: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) version: i64,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct OAuthClientViewRow {
    pub(crate) client_id: Uuid,
    pub(crate) name: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) tos_uri: String,
    pub(crate) policy_uri: String,
    pub(crate) client_uri: String,
    pub(crate) logo_uri: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct AuthorizationCodeRow {
    pub(crate) code: Uuid,
    pub(crate) client_id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) redirect_uri: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) code_challenge: String,
    pub(crate) code_challenge_method: String,
    pub(crate) expires: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct ThirdPartyExchangeCodeRow {
    pub(crate) code: Uuid,
    pub(crate) access_token_id: Uuid,
    pub(crate) access_token: String,
    pub(crate) access_token_expires: Option<OffsetDateTime>,
    pub(crate) scopes: Vec<String>,
    pub(crate) expires: OffsetDateTime,
}

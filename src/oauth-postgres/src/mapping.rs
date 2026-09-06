use crate::rows::{
    AuthorizationCodeRow, OAuthClientRow, OAuthClientViewRow, ThirdPartyExchangeCodeRow,
};
use credential_core::oauth_client_id::OAuthClientId;
use credential_core::scope::Scope;
use oauth_core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
    RehydratedAuthorizationCodeState,
};
use oauth_core::client::{
    InvalidOAuthRedirectUris, OAuthClient, OAuthClientName, OAuthRedirectUris,
    RehydratedOAuthClientState,
};
use oauth_core::third_party_exchange_code::{
    RehydratedThirdPartyExchangeCodeGrantState, ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant,
};
use oauth_service::ports::{OAuthClientStorageVersion, PersistedOAuthClient, VersionedOAuthClient};
use std::collections::HashSet;
use user_core::access_token::{AccessTokenId, HashedRawOAuthClientSecret, RawAccessToken};
use user_core::user_id::UserId;
use uuid::Uuid;

pub(crate) const OAUTH_CLIENT_COLUMNS: &str = "\
    client_id, client_secret_short_token AS secret_short, \
    client_secret_long_token_hash AS secret_hash, name, redirect_uris, tos_uri, policy_uri, \
    client_uri, logo_uri, scopes, version, created, updated";
pub(crate) const OAUTH_CLIENT_VIEW_COLUMNS: &str = "\
    client_id, name, redirect_uris, tos_uri, policy_uri, client_uri, logo_uri, scopes, created, \
    updated";
pub(crate) const AUTHORIZATION_CODE_COLUMNS: &str = "\
    authorization_code AS code, client_id, user_id, redirect_uri, scopes, code_challenge, \
    code_challenge_method, expires_at AS expires";
pub(crate) const THIRD_PARTY_EXCHANGE_CODE_COLUMNS: &str = "\
    third_party_exchange_code AS code, access_token_id, access_token, \
    access_token_expires_at AS access_token_expires, scopes, expires_at AS expires";

#[allow(clippy::enum_variant_names)]
#[derive(Debug, thiserror::Error)]
pub(crate) enum OAuthRowMappingError {
    #[error("persisted OAuth URL is invalid")]
    InvalidUrl(#[source] url::ParseError),
    #[error("persisted OAuth redirect URI set is invalid")]
    InvalidRedirectUris(#[source] InvalidOAuthRedirectUris),
    #[error("persisted OAuth scope is invalid: {0}")]
    InvalidScope(String),
    #[error("persisted OAuth code challenge method is invalid: {0}")]
    InvalidCodeChallengeMethod(String),
    #[error("persisted OAuth access token is invalid")]
    InvalidAccessToken(#[source] user_core::access_token::InvalidRawTokenError),
    #[error("OAuth identifier conversion failed")]
    InvalidIdentifier(#[source] uuid::Error),
    #[error("persisted OAuth client version is invalid")]
    InvalidVersion(#[from] domain_primitives::version::InvalidVersionError),
}

impl TryFrom<OAuthClientRow> for VersionedOAuthClient {
    type Error = OAuthRowMappingError;

    fn try_from(row: OAuthClientRow) -> Result<Self, Self::Error> {
        let version = OAuthClientStorageVersion::try_from(row.version)?;
        let client = OAuthClient::rehydrate(RehydratedOAuthClientState {
            client_id: OAuthClientId::from(row.client_id),
            hashed_client_secret: HashedRawOAuthClientSecret::new(
                row.secret_short,
                row.secret_hash,
            ),
            name: OAuthClientName::from(row.name),
            redirect_uris: parse_redirect_uris(row.redirect_uris)?,
            tos_uri: url::Url::parse(&row.tos_uri).map_err(OAuthRowMappingError::InvalidUrl)?,
            policy_uri: url::Url::parse(&row.policy_uri)
                .map_err(OAuthRowMappingError::InvalidUrl)?,
            client_uri: url::Url::parse(&row.client_uri)
                .map_err(OAuthRowMappingError::InvalidUrl)?,
            logo_uri: url::Url::parse(&row.logo_uri).map_err(OAuthRowMappingError::InvalidUrl)?,
            scopes: parse_scopes(row.scopes)?,
        });

        Ok(PersistedOAuthClient {
            value: client,
            version,
            created: row.created,
            updated: row.updated,
        })
    }
}

impl TryFrom<OAuthClientViewRow> for oauth_service::ports::OAuthClientView {
    type Error = OAuthRowMappingError;

    fn try_from(row: OAuthClientViewRow) -> Result<Self, Self::Error> {
        Ok(Self {
            client_id: OAuthClientId::from(row.client_id),
            name: OAuthClientName::from(row.name),
            redirect_uris: parse_redirect_uris(row.redirect_uris)?.into_set(),
            tos_uri: url::Url::parse(&row.tos_uri).map_err(OAuthRowMappingError::InvalidUrl)?,
            policy_uri: url::Url::parse(&row.policy_uri)
                .map_err(OAuthRowMappingError::InvalidUrl)?,
            client_uri: url::Url::parse(&row.client_uri)
                .map_err(OAuthRowMappingError::InvalidUrl)?,
            logo_uri: url::Url::parse(&row.logo_uri).map_err(OAuthRowMappingError::InvalidUrl)?,
            scopes: parse_scopes(row.scopes)?,
            created: row.created,
            updated: row.updated,
        })
    }
}

impl TryFrom<AuthorizationCodeRow> for AuthorizationCode {
    type Error = OAuthRowMappingError;

    fn try_from(row: AuthorizationCodeRow) -> Result<Self, Self::Error> {
        Ok(AuthorizationCode::rehydrate(
            RehydratedAuthorizationCodeState {
                code: OAuthAuthorizationCode::from(row.code),
                client_id: OAuthClientId::from(row.client_id),
                user_id: UserId::from(row.user_id),
                redirect_uri: url::Url::parse(&row.redirect_uri)
                    .map_err(OAuthRowMappingError::InvalidUrl)?,
                scopes: parse_scopes(row.scopes)?,
                code_challenge: OAuthCodeChallenge::from(row.code_challenge),
                code_challenge_method: parse_code_challenge_method(&row.code_challenge_method)?,
                expires: row.expires,
            },
        ))
    }
}

impl TryFrom<ThirdPartyExchangeCodeRow> for ThirdPartyExchangeCodeGrant {
    type Error = OAuthRowMappingError;

    fn try_from(row: ThirdPartyExchangeCodeRow) -> Result<Self, Self::Error> {
        Ok(ThirdPartyExchangeCodeGrant::rehydrate(
            RehydratedThirdPartyExchangeCodeGrantState {
                code: ThirdPartyExchangeCode::from(row.code),
                access_token_id: AccessTokenId::from(row.access_token_id),
                access_token: RawAccessToken::try_from(row.access_token)
                    .map_err(OAuthRowMappingError::InvalidAccessToken)?,
                access_token_expires: row.access_token_expires,
                scopes: parse_scopes(row.scopes)?,
                expires: row.expires,
            },
        ))
    }
}

pub(crate) fn client_id_uuid(client_id: &OAuthClientId) -> Result<Uuid, OAuthRowMappingError> {
    Uuid::parse_str(&client_id.to_string()).map_err(OAuthRowMappingError::InvalidIdentifier)
}

pub(crate) fn authorization_code_uuid(
    code: &OAuthAuthorizationCode,
) -> Result<Uuid, OAuthRowMappingError> {
    Uuid::parse_str(&code.to_string()).map_err(OAuthRowMappingError::InvalidIdentifier)
}

pub(crate) fn third_party_exchange_code_uuid(
    code: &ThirdPartyExchangeCode,
) -> Result<Uuid, OAuthRowMappingError> {
    Uuid::parse_str(&code.to_string()).map_err(OAuthRowMappingError::InvalidIdentifier)
}

pub(crate) fn access_token_id_uuid(
    access_token_id: AccessTokenId,
) -> Result<Uuid, OAuthRowMappingError> {
    Uuid::parse_str(&access_token_id.to_string()).map_err(OAuthRowMappingError::InvalidIdentifier)
}

pub(crate) fn scope_values(scopes: &HashSet<Scope>) -> Vec<String> {
    scopes.iter().copied().map(scope_to_db).collect()
}

fn parse_urls(values: Vec<String>) -> Result<HashSet<url::Url>, OAuthRowMappingError> {
    values
        .into_iter()
        .map(|value| url::Url::parse(&value).map_err(OAuthRowMappingError::InvalidUrl))
        .collect()
}

fn parse_redirect_uris(values: Vec<String>) -> Result<OAuthRedirectUris, OAuthRowMappingError> {
    OAuthRedirectUris::try_from(parse_urls(values)?)
        .map_err(OAuthRowMappingError::InvalidRedirectUris)
}

fn parse_scopes(values: Vec<String>) -> Result<HashSet<Scope>, OAuthRowMappingError> {
    values
        .into_iter()
        .map(|value| scope_from_db(&value))
        .collect()
}

fn scope_to_db(scope: Scope) -> String {
    scope.as_str().to_owned()
}

fn scope_from_db(value: &str) -> Result<Scope, OAuthRowMappingError> {
    match value {
        "product-listings:write" => Ok(Scope::ProductListingsWrite),
        "users:read" => Ok(Scope::UsersRead),
        "users:write" => Ok(Scope::UsersWrite),
        "access-tokens:read" => Ok(Scope::AccessTokensRead),
        "access-tokens:write" => Ok(Scope::AccessTokensWrite),
        "search-filters:write" => Ok(Scope::SearchFiltersWrite),
        "watchlist:read" => Ok(Scope::WatchlistRead),
        "watchlist:write" => Ok(Scope::WatchlistWrite),
        _ => Err(OAuthRowMappingError::InvalidScope(value.to_owned())),
    }
}

fn parse_code_challenge_method(value: &str) -> Result<CodeChallengeMethod, OAuthRowMappingError> {
    match value {
        "S256" => Ok(CodeChallengeMethod::S256),
        _ => Err(OAuthRowMappingError::InvalidCodeChallengeMethod(
            value.to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_rehydrate_versioned_oauth_client_with_operational_metadata() {
        let row = OAuthClientRow {
            client_id: Uuid::nil(),
            secret_short: "short".to_owned(),
            secret_hash: "hash".to_owned(),
            name: "Client".to_owned(),
            redirect_uris: vec!["https://client.example/callback".to_owned()],
            tos_uri: "https://client.example/tos".to_owned(),
            policy_uri: "https://client.example/policy".to_owned(),
            client_uri: "https://client.example".to_owned(),
            logo_uri: "https://client.example/logo.png".to_owned(),
            scopes: vec!["product-listings:write".to_owned()],
            version: 1,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        };

        let persisted = match VersionedOAuthClient::try_from(row) {
            Ok(persisted) => persisted,
            Err(error) => panic!("persisted OAuth client must map: {error}"),
        };

        assert_eq!(OAuthClientStorageVersion::INITIAL, persisted.version);
        assert_eq!(&OAuthClientName::from("Client"), persisted.value.name());
        assert_eq!(1, persisted.value.redirect_uris().as_set().len());
        assert_eq!(time::OffsetDateTime::UNIX_EPOCH, persisted.created);
        assert_eq!(time::OffsetDateTime::UNIX_EPOCH, persisted.updated);
    }

    #[test]
    fn should_reject_http_redirect_uri_in_persisted_oauth_client() {
        let row = OAuthClientRow {
            client_id: Uuid::nil(),
            secret_short: "short".to_owned(),
            secret_hash: "hash".to_owned(),
            name: "Client".to_owned(),
            redirect_uris: vec!["http://client.example/callback".to_owned()],
            tos_uri: "https://client.example/tos".to_owned(),
            policy_uri: "https://client.example/policy".to_owned(),
            client_uri: "https://client.example".to_owned(),
            logo_uri: "https://client.example/logo.png".to_owned(),
            scopes: vec!["product-listings:write".to_owned()],
            version: 1,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        };

        assert!(matches!(
            VersionedOAuthClient::try_from(row),
            Err(OAuthRowMappingError::InvalidRedirectUris(_))
        ));
    }

    #[test]
    fn should_reject_fragment_redirect_uri_in_persisted_oauth_client() {
        let row = OAuthClientRow {
            client_id: Uuid::nil(),
            secret_short: "short".to_owned(),
            secret_hash: "hash".to_owned(),
            name: "Client".to_owned(),
            redirect_uris: vec!["https://client.example/callback#fragment".to_owned()],
            tos_uri: "https://client.example/tos".to_owned(),
            policy_uri: "https://client.example/policy".to_owned(),
            client_uri: "https://client.example".to_owned(),
            logo_uri: "https://client.example/logo.png".to_owned(),
            scopes: vec!["product-listings:write".to_owned()],
            version: 1,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        };

        assert!(matches!(
            VersionedOAuthClient::try_from(row),
            Err(OAuthRowMappingError::InvalidRedirectUris(_))
        ));
    }

    #[test]
    fn should_reject_invalid_oauth_client_storage_version() {
        let row = OAuthClientRow {
            client_id: Uuid::nil(),
            secret_short: "short".to_owned(),
            secret_hash: "hash".to_owned(),
            name: "Client".to_owned(),
            redirect_uris: vec!["https://client.example/callback".to_owned()],
            tos_uri: "https://client.example/tos".to_owned(),
            policy_uri: "https://client.example/policy".to_owned(),
            client_uri: "https://client.example".to_owned(),
            logo_uri: "https://client.example/logo.png".to_owned(),
            scopes: vec!["product-listings:write".to_owned()],
            version: 0,
            created: time::OffsetDateTime::UNIX_EPOCH,
            updated: time::OffsetDateTime::UNIX_EPOCH,
        };

        assert!(matches!(
            VersionedOAuthClient::try_from(row),
            Err(OAuthRowMappingError::InvalidVersion(_))
        ));
    }
}

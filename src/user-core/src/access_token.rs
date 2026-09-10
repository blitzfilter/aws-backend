use crate::user_id::UserId;
use credential_core::oauth_client_id::OAuthClientId;
use domain_primitives::{object_id_newtype, string_newtype};
use prefixed_api_key::{
    PrefixedApiKey, PrefixedApiKeyController,
    sha2::{Digest, Sha256},
};
use std::collections::HashSet;
use std::marker::PhantomData;
use time::OffsetDateTime;

object_id_newtype!(AccessTokenId, "at");
string_newtype!(AccessTokenName, max_length(128));

pub use credential_core::scope::Scope;

#[derive(Debug, Clone, PartialEq)]
pub struct AccessToken {
    id: AccessTokenId,
    hashed_token: HashedRawAccessToken,
    user_id: UserId,
    name: AccessTokenName,
    scopes: HashSet<Scope>,
    origin: AccessTokenOrigin,
    expires: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewAccessToken {
    pub id: AccessTokenId,
    pub hashed_token: HashedRawAccessToken,
    pub user_id: UserId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub origin: AccessTokenOrigin,
    pub expires: Option<OffsetDateTime>,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedAccessTokenState {
    pub id: AccessTokenId,
    pub hashed_token: HashedRawAccessToken,
    pub user_id: UserId,
    pub name: AccessTokenName,
    pub scopes: HashSet<Scope>,
    pub origin: AccessTokenOrigin,
    pub expires: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessTokenOrigin {
    User,
    OAuth { client_id: OAuthClientId },
}

impl AccessToken {
    pub fn create(input: NewAccessToken) -> Self {
        Self {
            id: input.id,
            hashed_token: input.hashed_token,
            user_id: input.user_id,
            name: input.name,
            scopes: input.scopes,
            origin: input.origin,
            expires: input.expires,
        }
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedAccessTokenState) -> Self {
        Self {
            id: state.id,
            hashed_token: state.hashed_token,
            user_id: state.user_id,
            name: state.name,
            scopes: state.scopes,
            origin: state.origin,
            expires: state.expires,
        }
    }

    pub fn change_name(&mut self, name: AccessTokenName) -> bool {
        if self.name == name {
            false
        } else {
            self.name = name;
            true
        }
    }

    pub fn replace_scopes(&mut self, scopes: HashSet<Scope>) -> bool {
        if self.scopes == scopes {
            false
        } else {
            self.scopes = scopes;
            true
        }
    }

    pub fn change_expires(&mut self, expires: Option<OffsetDateTime>) -> bool {
        if self.expires == expires {
            false
        } else {
            self.expires = expires;
            true
        }
    }

    pub fn is_expired_at(&self, now: OffsetDateTime) -> bool {
        self.expires.is_some_and(|expires| expires < now)
    }

    pub fn has_scope(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }

    pub fn id(&self) -> AccessTokenId {
        self.id
    }

    pub fn hashed_token(&self) -> &HashedRawAccessToken {
        &self.hashed_token
    }

    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn name(&self) -> &AccessTokenName {
        &self.name
    }

    pub fn scopes(&self) -> &HashSet<Scope> {
        &self.scopes
    }

    pub fn origin(&self) -> &AccessTokenOrigin {
        &self.origin
    }

    pub fn expires(&self) -> Option<OffsetDateTime> {
        self.expires
    }
}

// ── TokenPrefix trait and prefix types ─────────────────────────────────────

pub trait TokenPrefix: std::fmt::Debug + Clone + PartialEq + Send + Sync + 'static {
    const PREFIX: &'static str;
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccessTokenPrefix;

impl TokenPrefix for AccessTokenPrefix {
    const PREFIX: &'static str = "aurahistoria_accesstoken";
}

#[derive(Debug, Clone, PartialEq)]
pub struct OAuthClientSecretPrefix;

impl TokenPrefix for OAuthClientSecretPrefix {
    const PREFIX: &'static str = "aurahistoria_oauth_client_secret";
}

// ── Type aliases ────────────────────────────────────────────────────────────

pub type RawAccessToken = RawToken<AccessTokenPrefix>;
pub type RawOAuthClientSecret = RawToken<OAuthClientSecretPrefix>;
pub type HashedRawAccessToken = HashedRawToken<AccessTokenPrefix>;
pub type HashedRawOAuthClientSecret = HashedRawToken<OAuthClientSecretPrefix>;

// ── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
#[error("RawToken '{0}' is invalid, because {1}")]
pub struct InvalidRawTokenError(String, String);

/// Backward-compatibility alias.
pub type InvalidRawAccessTokenError = InvalidRawTokenError;

// ── RawToken<P> ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct RawToken<P: TokenPrefix> {
    value: String,
    _marker: PhantomData<P>,
}

impl<P: TokenPrefix> RawToken<P> {
    pub fn new() -> Self {
        let key = PrefixedApiKeyController::configure()
            .prefix(P::PREFIX.to_owned())
            .seam_defaults()
            .finalize()
            .expect("shouldn't fail creating PrefixedApiKeyController because all required fields are set")
            .generate_key()
            .to_string();
        Self {
            value: key,
            _marker: PhantomData,
        }
    }

    pub fn check(&self, hashed: &HashedRawToken<P>) -> bool {
        PrefixedApiKeyController::configure()
            .prefix(P::PREFIX.to_owned())
            .seam_defaults()
            .finalize()
            .expect("shouldn't fail creating PrefixedApiKeyController because all required fields are set")
            .check_hash(&self.into(), hashed.long_token_hash())
    }
}

impl<P: TokenPrefix> Default for RawToken<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: TokenPrefix> From<RawToken<P>> for String {
    fn from(value: RawToken<P>) -> Self {
        value.value
    }
}

impl<P: TokenPrefix> TryFrom<String> for RawToken<P> {
    type Error = InvalidRawTokenError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let _ = parse_token(&value, P::PREFIX)?;
        Ok(Self {
            value,
            _marker: PhantomData,
        })
    }
}

#[cfg(any())]
pub mod api {
    use crate::access_token::{AccessTokenId, RawAccessToken};
    use common::{
        api::{
            error::ApiError,
            error_code::{BAD_HEADER_VALUE, BAD_PATH_PARAMETER_VALUE, INVALID_UUID},
        },
        error::missing_field::MissingRequiredField,
    };
    use http::{HeaderMap, header::AUTHORIZATION};
    use std::collections::HashMap;

    #[derive(Debug, thiserror::Error)]
    pub enum ExtractBearerTokenError {
        #[error("Invalid authorization header value: '{0}'")]
        InvalidHeaderValue(#[from] http::header::ToStrError),
        #[error("Token in authorization header is not a valid bearer token")]
        InvalidBearerTokenFormat,
    }

    impl From<ExtractBearerTokenError> for ApiError {
        fn from(value: ExtractBearerTokenError) -> Self {
            match value {
                ExtractBearerTokenError::InvalidHeaderValue(err) => {
                    let msg = err.to_string();
                    ApiError::bad_request(BAD_HEADER_VALUE, Box::new(err))
                        .with_header_field(AUTHORIZATION.as_str())
                        .with_detail(msg)
                }
                err @ ExtractBearerTokenError::InvalidBearerTokenFormat => {
                    let msg = err.to_string();
                    ApiError::bad_request(BAD_HEADER_VALUE, Box::new(err))
                        .with_header_field(AUTHORIZATION.as_str())
                        .with_detail(msg)
                }
            }
        }
    }

    pub fn extract_bearer_token(
        headers: &HeaderMap,
    ) -> Result<Option<String>, ExtractBearerTokenError> {
        let authorization = headers
            .get(AUTHORIZATION)
            .map(|value| value.to_str())
            .transpose()?;

        match authorization {
            None => Ok(None),
            Some(value) => value
                .strip_prefix("Bearer ")
                .map(ToOwned::to_owned)
                .ok_or(ExtractBearerTokenError::InvalidBearerTokenFormat)
                .map(Some),
        }
    }

    pub fn extract_bearer_access_token(
        headers: &HeaderMap,
    ) -> Result<Option<RawAccessToken>, ApiError> {
        extract_bearer_token(headers)?
            .map(|value| {
                RawAccessToken::try_from(value).map_err(|err| {
                    ApiError::unauthorized(common::api::error_code::UNAUTHORIZED)
                        .with_header_field(AUTHORIZATION.as_str())
                        .with_detail(err.to_string())
                })
            })
            .transpose()
    }

    pub fn extract_access_token_id_path(
        path_params: &HashMap<String, String>,
    ) -> Result<AccessTokenId, ApiError> {
        path_params
            .get("accessTokenId")
            .map(AccessTokenId::try_from)
            .transpose()
            .map_err(|err| {
                let msg = err.to_string();
                ApiError::bad_request(INVALID_UUID, Box::new(err))
                    .with_path_field("accessTokenId")
                    .with_detail(msg)
            })?
            .ok_or(
                ApiError::bad_request(
                    BAD_PATH_PARAMETER_VALUE,
                    Box::new(MissingRequiredField::new("accessTokenId")),
                )
                .with_path_field("accessTokenId")
                .with_detail("Missing field 'accessTokenId'."),
            )
    }
}

#[cfg(all(test, any()))]
mod api_tests {
    use super::*;
    use crate::access_token::api::{
        extract_access_token_id_path, extract_bearer_access_token, extract_bearer_token,
    };
    use http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
    use std::collections::HashMap;

    #[test]
    fn should_return_none_when_authorization_header_missing() {
        let headers = HeaderMap::new();

        assert!(matches!(extract_bearer_token(&headers), Ok(None)));
    }

    #[test]
    fn should_extract_bearer_token_when_authorization_header_valid() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer token-value"),
        );

        assert!(
            matches!(extract_bearer_token(&headers), Ok(Some(token)) if token == "token-value")
        );
    }

    #[test]
    fn should_reject_authorization_header_when_not_bearer() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Basic token-value"));

        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn should_reject_authorization_header_when_value_not_visible_ascii() {
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_bytes(b"Bearer \xff")
            .unwrap_or_else(|error| panic!("invalid test header value: {error}"));
        headers.insert(AUTHORIZATION, value);

        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn should_extract_bearer_access_token_when_header_valid() {
        let key = RawAccessToken::new();
        let key_string = String::from(key.clone());
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_str(&format!("Bearer {key_string}"))
            .unwrap_or_else(|error| panic!("invalid test header value: {error}"));
        headers.insert(AUTHORIZATION, value);

        let result = extract_bearer_access_token(&headers);

        assert!(matches!(result, Ok(Some(token)) if token == key));
    }

    #[test]
    fn should_return_none_when_bearer_access_token_missing() {
        let headers = HeaderMap::new();

        assert!(matches!(extract_bearer_access_token(&headers), Ok(None)));
    }

    #[test]
    fn should_reject_bearer_access_token_when_token_invalid() {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer invalid"));

        assert!(extract_bearer_access_token(&headers).is_err());
    }

    #[test]
    fn should_extract_access_token_id_from_path() {
        let id = AccessTokenId::new();
        let mut path_params = HashMap::new();
        path_params.insert("accessTokenId".to_string(), id.to_string());

        assert!(matches!(extract_access_token_id_path(&path_params), Ok(value) if value == id));
    }

    #[test]
    fn should_reject_access_token_id_path_when_missing() {
        let path_params = HashMap::new();

        assert!(extract_access_token_id_path(&path_params).is_err());
    }

    #[test]
    fn should_reject_access_token_id_path_when_invalid() {
        let mut path_params = HashMap::new();
        path_params.insert("accessTokenId".to_string(), "not-a-uuid".to_string());

        assert!(extract_access_token_id_path(&path_params).is_err());
    }
}

// `PrefixedApiKey::from_string` requires exactly three `_`-delimited parts, so
// it cannot handle prefixes that themselves contain underscores.  We parse with
// `parse_token` instead and construct via `PrefixedApiKey::new`.
impl<P: TokenPrefix> From<&RawToken<P>> for PrefixedApiKey {
    fn from(value: &RawToken<P>) -> Self {
        let (short_token, long_token) = parse_token(&value.value, P::PREFIX)
            .expect("shouldn't fail parsing RawToken as PrefixedApiKey by construction");
        PrefixedApiKey::new(P::PREFIX.to_owned(), short_token, long_token)
    }
}

fn parse_token(token: &str, prefix: &str) -> Result<(String, String), InvalidRawTokenError> {
    let (short_token, long_token) = token
        .strip_prefix(prefix)
        .ok_or_else(|| {
            InvalidRawTokenError(
                token.to_string(),
                format!("it doesn't start with the required prefix '{prefix}'"),
            )
        })?
        .strip_prefix('_')
        .ok_or_else(|| {
            InvalidRawTokenError(
                token.to_string(),
                format!("it should contain a '_' after prefix '{prefix}'"),
            )
        })?
        .split_once('_')
        .ok_or_else(|| {
            InvalidRawTokenError(
                token.to_string(),
                "it should contain a '_' separating the short and long token".to_string(),
            )
        })?;

    Ok((short_token.to_string(), long_token.to_string()))
}

// ── HashedRawToken<P> ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct HashedRawToken<P: TokenPrefix> {
    short_token: String,
    long_token_hash: String,
    _marker: PhantomData<P>,
}

impl<P: TokenPrefix> std::fmt::Display for HashedRawToken<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}_{}_****", P::PREFIX, self.short_token)
    }
}

impl<P: TokenPrefix> HashedRawToken<P> {
    pub fn new(short_token: String, long_token_hash: String) -> Self {
        Self {
            short_token,
            long_token_hash,
            _marker: PhantomData,
        }
    }

    pub fn prefix(&self) -> &'static str {
        P::PREFIX
    }

    pub fn short_token(&self) -> &str {
        &self.short_token
    }

    pub fn long_token_hash(&self) -> &str {
        &self.long_token_hash
    }
}

impl<P: TokenPrefix> From<RawToken<P>> for HashedRawToken<P> {
    fn from(value: RawToken<P>) -> Self {
        let pak: PrefixedApiKey = (&value).into();
        pak.into()
    }
}

impl<P: TokenPrefix> From<PrefixedApiKey> for HashedRawToken<P> {
    fn from(value: PrefixedApiKey) -> Self {
        let mut digest = Sha256::new();
        Self {
            short_token: value.short_token().to_string(),
            long_token_hash: value.long_token_hashed(&mut digest),
            _marker: PhantomData,
        }
    }
}

#[cfg(test)]
mod access_token_state_tests {
    use super::*;

    #[test]
    fn should_report_not_expired_when_no_expiry() {
        let token = access_token(None, HashSet::new());

        assert!(!token.is_expired_at(OffsetDateTime::now_utc()));
    }

    #[test]
    fn should_report_not_expired_when_expiry_in_future() {
        let token = access_token(
            Some(OffsetDateTime::now_utc() + time::Duration::days(1)),
            HashSet::new(),
        );

        assert!(!token.is_expired_at(OffsetDateTime::now_utc()));
    }

    #[test]
    fn should_report_expired_when_expiry_in_past() {
        let token = access_token(
            Some(OffsetDateTime::now_utc() - time::Duration::days(1)),
            HashSet::new(),
        );

        assert!(token.is_expired_at(OffsetDateTime::now_utc()));
    }

    #[test]
    fn should_render_all_scope_strings() {
        for (scope, value) in [
            (Scope::ProductListingsWrite, "product-listings:write"),
            (Scope::UsersRead, "users:read"),
            (Scope::UsersWrite, "users:write"),
            (Scope::AccessTokensRead, "access-tokens:read"),
            (Scope::AccessTokensWrite, "access-tokens:write"),
            (Scope::SearchFiltersWrite, "search-filters:write"),
            (Scope::WatchlistRead, "watchlist:read"),
            (Scope::WatchlistWrite, "watchlist:write"),
        ] {
            assert_eq!(value, scope.as_str());
        }
    }

    #[test]
    fn should_report_scope_presence() {
        let token = access_token(None, HashSet::from([Scope::ProductListingsWrite]));

        assert!(token.has_scope(Scope::ProductListingsWrite));
        assert!(!token.has_scope(Scope::UsersWrite));
    }

    #[test]
    fn should_preserve_oauth_origin_client_id() {
        let client_id = OAuthClientId::new();
        let mut token = access_token(None, HashSet::new());
        token.origin = AccessTokenOrigin::OAuth { client_id };

        assert_eq!(AccessTokenOrigin::OAuth { client_id }, token.origin);
    }

    fn access_token(expires: Option<OffsetDateTime>, scopes: HashSet<Scope>) -> AccessToken {
        AccessToken {
            id: AccessTokenId::new(),
            hashed_token: RawAccessToken::new().into(),
            user_id: UserId::new(),
            name: AccessTokenName::from("test"),
            scopes,
            origin: AccessTokenOrigin::User,
            expires,
        }
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Dummy, Fake, Faker, RngExt};

    impl Dummy<Faker> for RawAccessToken {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawAccessToken::new()
        }
    }

    impl Dummy<Faker> for RawOAuthClientSecret {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawOAuthClientSecret::new()
        }
    }

    impl Dummy<Faker> for HashedRawAccessToken {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawAccessToken::new().into()
        }
    }

    impl Dummy<Faker> for HashedRawOAuthClientSecret {
        fn dummy_with_rng<R: RngExt + ?Sized>(_config: &Faker, _rng: &mut R) -> Self {
            RawOAuthClientSecret::new().into()
        }
    }

    impl Dummy<Faker> for AccessToken {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            AccessToken {
                id: config.fake_with_rng(rng),
                hashed_token: config.fake_with_rng(rng),
                user_id: config.fake_with_rng(rng),
                name: config.fake_with_rng(rng),
                scopes: [Scope::ProductListingsWrite].into(),
                origin: AccessTokenOrigin::User,
                expires: None,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::access_token::{
            HashedRawAccessToken, HashedRawOAuthClientSecret, RawAccessToken, RawOAuthClientSecret,
        };
        use fake::{Fake, Faker};

        #[test]
        fn should_fake_access_token() {
            for _ in 0..100 {
                let _ = Faker.fake::<RawAccessToken>();
            }
        }

        #[test]
        fn should_fake_hashed_access_token() {
            for _ in 0..100 {
                let _ = Faker.fake::<HashedRawAccessToken>();
            }
        }

        #[test]
        fn should_fake_oauth_client_secret() {
            for _ in 0..100 {
                let _ = Faker.fake::<RawOAuthClientSecret>();
            }
        }

        #[test]
        fn should_fake_hashed_oauth_client_secret() {
            for _ in 0..100 {
                let _ = Faker.fake::<HashedRawOAuthClientSecret>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // ── Prefix starts ─────────────────────────────────────────────────────

    #[test]
    fn should_start_with_access_token_prefix_when_new_for_access_token() {
        let key: String = RawAccessToken::new().into();
        assert!(key.starts_with("aurahistoria_accesstoken_"));
    }

    #[test]
    fn should_start_with_oauth_secret_prefix_when_new_for_oauth_secret() {
        let key: String = RawOAuthClientSecret::new().into();
        assert!(key.starts_with("aurahistoria_oauth_client_secret_"));
    }

    // ── Token lengths ─────────────────────────────────────────────────────

    #[test]
    fn should_have_correct_token_lengths_when_new_for_access_token() {
        // seam_defaults() uses 8 bytes for short (base58 → 10-11 chars)
        // and 24 bytes for long (base58 → 32-33 chars); exact length depends on leading zeros
        let key: String = RawAccessToken::new().into();
        let stripped = key.strip_prefix("aurahistoria_accesstoken_").unwrap();
        let (short, long) = stripped.split_once('_').unwrap();
        assert!(
            (10..=11).contains(&short.len()),
            "short token should be 10-11 characters, was {}",
            short.len()
        );
        assert!(
            (32..=33).contains(&long.len()),
            "long token should be 32-33 characters, was {}",
            long.len()
        );
    }

    #[test]
    fn should_have_correct_token_lengths_when_new_for_oauth_secret() {
        let key: String = RawOAuthClientSecret::new().into();
        let stripped = key
            .strip_prefix("aurahistoria_oauth_client_secret_")
            .unwrap();
        let (short, long) = stripped.split_once('_').unwrap();
        assert!(
            (10..=11).contains(&short.len()),
            "short token should be 10-11 characters, was {}",
            short.len()
        );
        assert!(
            (32..=33).contains(&long.len()),
            "long token should be 32-33 characters, was {}",
            long.len()
        );
    }

    // ── Uniqueness ────────────────────────────────────────────────────────

    #[test]
    fn should_generate_unique_keys_when_called_multiple_times_for_access_token() {
        let keys: Vec<String> = (0..10).map(|_| RawAccessToken::new().into()).collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 10);
    }

    #[test]
    fn should_generate_unique_keys_when_called_multiple_times_for_oauth_secret() {
        let keys: Vec<String> = (0..10)
            .map(|_| RawOAuthClientSecret::new().into())
            .collect();
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 10);
    }

    // ── Default ───────────────────────────────────────────────────────────

    #[test]
    fn should_produce_valid_parseable_key_when_default_for_access_token() {
        let key = RawAccessToken::default();
        let key_str: String = key.into();
        let result = RawAccessToken::try_from(key_str);
        assert!(result.is_ok());
    }

    #[test]
    fn should_produce_valid_parseable_key_when_default_for_oauth_secret() {
        let key = RawOAuthClientSecret::default();
        let key_str: String = key.into();
        let result = RawOAuthClientSecret::try_from(key_str);
        assert!(result.is_ok());
    }

    #[test]
    fn should_create_distinct_at_object_ids_when_default() {
        let first = AccessTokenId::default();
        let second = AccessTokenId::default();

        assert_ne!(first, second);
        assert_eq!("at", AccessTokenId::PREFIX);
        assert_eq!(7, first.as_uuid().get_version_num());
        assert!(first.to_string().starts_with("at_"));
    }

    #[test]
    fn should_use_at_object_id_contract() -> Result<(), Box<dyn std::error::Error>> {
        const UUID_TEXT: &str = "01890a5d-ac96-774b-bf1d-d5586c639f75";
        const TYPE_ID_SUFFIX: &str = "01h455vb4pex5vy7enb1p677vn";

        let uuid = uuid::Uuid::parse_str(UUID_TEXT)?;
        let id = AccessTokenId::try_from(uuid)?;

        assert_eq!(format!("at_{TYPE_ID_SUFFIX}"), id.to_string());
        assert_eq!(uuid, id.into_uuid());
        assert!(AccessTokenId::try_from(UUID_TEXT).is_err());
        assert!(AccessTokenId::try_from(format!("usr_{TYPE_ID_SUFFIX}")).is_err());

        Ok(())
    }

    // ── TryFrom<String>: valid cases ──────────────────────────────────────

    #[rstest]
    #[case("aurahistoria_accesstoken_12345678901_123456789012345678901234567890123")]
    #[case("aurahistoria_accesstoken_abcdefghijk_abcdefghijklmnopqrstuvwxyz1234567")]
    #[trace]
    fn should_parse_valid_key_when_try_from_string_for_access_token(#[case] input: &str) {
        let result = RawAccessToken::try_from(input.to_string());
        assert!(result.is_ok());
    }

    #[rstest]
    #[case("aurahistoria_oauth_client_secret_12345678901_123456789012345678901234567890123")]
    #[case("aurahistoria_oauth_client_secret_abcdefghijk_abcdefghijklmnopqrstuvwxyz1234567")]
    #[trace]
    fn should_parse_valid_key_when_try_from_string_for_oauth_secret(#[case] input: &str) {
        let result = RawOAuthClientSecret::try_from(input.to_string());
        assert!(result.is_ok());
    }

    // ── TryFrom<String>: invalid / rejected cases ─────────────────────────

    #[rstest]
    #[case::wrong_prefix("badprefix_12345678901_123456789012345678901234567890123")]
    #[case::old_prefix("aurahistoria_12345678901_123456789012345678901234567890123")]
    #[case::cross_type_oauth_prefix(
        "aurahistoria_oauth_client_secret_12345678901_123456789012345678901234567890123"
    )]
    #[case::no_underscore_after_prefix(
        "aurahistoria_accesstoken12345678901_123456789012345678901234567890123"
    )]
    #[case::no_token_separator(
        "aurahistoria_accesstoken_12345678901123456789012345678901234567890123"
    )]
    #[case::empty("")]
    #[trace]
    fn should_reject_invalid_key_when_try_from_string_for_access_token(#[case] input: &str) {
        let result = RawAccessToken::try_from(input.to_string());
        assert!(result.is_err());
    }

    #[rstest]
    #[case::wrong_prefix("badprefix_12345678901_123456789012345678901234567890123")]
    #[case::old_prefix("aurahistoria_12345678901_123456789012345678901234567890123")]
    #[case::cross_type_access_token_prefix(
        "aurahistoria_accesstoken_12345678901_123456789012345678901234567890123"
    )]
    #[case::no_token_separator(
        "aurahistoria_oauth_client_secret_12345678901123456789012345678901234567890123"
    )]
    #[case::empty("")]
    #[trace]
    fn should_reject_invalid_key_when_try_from_string_for_oauth_secret(#[case] input: &str) {
        let result = RawOAuthClientSecret::try_from(input.to_string());
        assert!(result.is_err());
    }

    // ── Round-trip ────────────────────────────────────────────────────────

    #[test]
    fn should_round_trip_through_string_when_from_and_try_from_for_access_token() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let restored = RawAccessToken::try_from(key_str).unwrap();
        assert_eq!(key, restored);
    }

    #[test]
    fn should_round_trip_through_string_when_from_and_try_from_for_oauth_secret() {
        let key = RawOAuthClientSecret::new();
        let key_str: String = key.clone().into();
        let restored = RawOAuthClientSecret::try_from(key_str).unwrap();
        assert_eq!(key, restored);
    }

    // ── check() ───────────────────────────────────────────────────────────

    #[test]
    fn should_return_true_when_checking_own_hash_for_access_token() {
        let key = RawAccessToken::new();
        let hash = HashedRawAccessToken::from(key.clone());
        assert!(key.check(&hash));
    }

    #[test]
    fn should_return_false_when_checking_hash_of_different_key_for_access_token() {
        let key1 = RawAccessToken::new();
        let key2 = RawAccessToken::new();
        let hash2 = HashedRawAccessToken::from(key2);
        assert!(!key1.check(&hash2));
    }

    #[test]
    fn should_return_true_when_checking_own_hash_for_oauth_secret() {
        let key = RawOAuthClientSecret::new();
        let hash = HashedRawOAuthClientSecret::from(key.clone());
        assert!(key.check(&hash));
    }

    #[test]
    fn should_return_false_when_checking_hash_of_different_key_for_oauth_secret() {
        let key1 = RawOAuthClientSecret::new();
        let key2 = RawOAuthClientSecret::new();
        let hash2 = HashedRawOAuthClientSecret::from(key2);
        assert!(!key1.check(&hash2));
    }

    // ── From<&RawToken<P>> for PrefixedApiKey ─────────────────────────────

    #[test]
    fn should_preserve_prefix_and_short_token_when_converting_to_prefixed_api_key() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let stripped = key_str.strip_prefix("aurahistoria_accesstoken_").unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let prefixed: PrefixedApiKey = (&key).into();

        assert_eq!(prefixed.prefix(), "aurahistoria_accesstoken");
        assert_eq!(prefixed.short_token(), expected_short);
    }

    // ── HashedRawToken::new and accessors ─────────────────────────────────

    #[test]
    fn should_store_all_fields_correctly_when_creating_via_new_for_access_token() {
        let short = "ABCDEFGH".to_string();
        let hash = "somehashvalue".to_string();

        let hashed = HashedRawAccessToken::new(short.clone(), hash.clone());

        assert_eq!(hashed.prefix(), AccessTokenPrefix::PREFIX);
        assert_eq!(hashed.short_token(), short);
        assert_eq!(hashed.long_token_hash(), hash);
    }

    #[test]
    fn should_store_all_fields_correctly_when_creating_via_new_for_oauth_secret() {
        let short = "ABCDEFGH".to_string();
        let hash = "somehashvalue".to_string();

        let hashed = HashedRawOAuthClientSecret::new(short.clone(), hash.clone());

        assert_eq!(hashed.prefix(), OAuthClientSecretPrefix::PREFIX);
        assert_eq!(hashed.short_token(), short);
        assert_eq!(hashed.long_token_hash(), hash);
    }

    // ── From<RawToken<P>> for HashedRawToken<P> ───────────────────────────

    #[test]
    fn should_produce_sha256_hash_when_converting_from_raw_token_for_access_token() {
        let key = RawAccessToken::new();
        let key_str: String = key.clone().into();
        let stripped = key_str.strip_prefix("aurahistoria_accesstoken_").unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let hashed = HashedRawAccessToken::from(key);

        assert_eq!(hashed.prefix(), AccessTokenPrefix::PREFIX);
        assert_eq!(hashed.short_token(), expected_short);
        assert_eq!(
            hashed.long_token_hash().len(),
            64,
            "SHA-256 hex digest should be 64 characters"
        );
    }

    #[test]
    fn should_produce_sha256_hash_when_converting_from_raw_token_for_oauth_secret() {
        let key = RawOAuthClientSecret::new();
        let key_str: String = key.clone().into();
        let stripped = key_str
            .strip_prefix("aurahistoria_oauth_client_secret_")
            .unwrap();
        let (expected_short, _) = stripped.split_once('_').unwrap();

        let hashed = HashedRawOAuthClientSecret::from(key);

        assert_eq!(hashed.prefix(), OAuthClientSecretPrefix::PREFIX);
        assert_eq!(hashed.short_token(), expected_short);
        assert_eq!(
            hashed.long_token_hash().len(),
            64,
            "SHA-256 hex digest should be 64 characters"
        );
    }

    // ── Equal / different hashes for access token ─────────────────────────

    #[test]
    fn should_produce_equal_hashes_when_converting_same_key_twice_for_access_token() {
        let key = RawAccessToken::new();
        let hash1 = HashedRawAccessToken::from(key.clone());
        let hash2 = HashedRawAccessToken::from(key);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn should_produce_different_hashes_when_converting_different_keys_for_access_token() {
        let key1 = RawAccessToken::new();
        let key2 = RawAccessToken::new();
        let hash1 = HashedRawAccessToken::from(key1);
        let hash2 = HashedRawAccessToken::from(key2);
        assert_ne!(hash1, hash2);
    }

    // ── From<PrefixedApiKey> for HashedRawToken ────────────────────────────

    #[test]
    fn should_produce_same_result_when_converting_from_prefixed_api_key_as_from_raw_token_for_access_token()
     {
        let key = RawAccessToken::new();
        let prefixed: PrefixedApiKey = (&key).into();

        let hash_from_key = HashedRawAccessToken::from(key);
        let hash_from_prefixed = HashedRawAccessToken::from(prefixed);

        assert_eq!(hash_from_key, hash_from_prefixed);
    }

    #[test]
    fn should_set_correct_prefix_when_converting_from_prefixed_api_key_for_access_token() {
        let key = RawAccessToken::new();
        let prefixed: PrefixedApiKey = (&key).into();
        let hashed = HashedRawAccessToken::from(prefixed);
        assert_eq!(hashed.prefix(), AccessTokenPrefix::PREFIX);
    }

    #[test]
    fn should_mask_long_token_when_displaying_hashed_access_token() {
        let hashed = HashedRawAccessToken::new("short".to_string(), "secret_hash".to_string());

        assert_eq!("aurahistoria_accesstoken_short_****", hashed.to_string());
    }

    // ── Error messages ────────────────────────────────────────────────────

    #[test]
    fn should_include_input_value_in_error_message_when_parsing_fails_for_access_token() {
        let bad_key = "totally_wrong_format";
        let err = RawAccessToken::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }

    #[test]
    fn should_include_input_value_in_error_message_when_parsing_fails_for_oauth_secret() {
        let bad_key = "totally_wrong_format";
        let err = RawOAuthClientSecret::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }

    #[rstest]
    #[case::wrong_prefix("wrongprefix_12345678901_123456789012345678901234567890123")]
    #[case::no_separator("aurahistoria_accesstoken_12345678901123456789012345678901234567890123")]
    #[trace]
    fn should_include_bad_key_in_error_message_for_each_validation_branch_for_access_token(
        #[case] bad_key: &str,
    ) {
        let err = RawAccessToken::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }

    #[rstest]
    #[case::wrong_prefix("wrongprefix_12345678901_123456789012345678901234567890123")]
    #[case::no_separator(
        "aurahistoria_oauth_client_secret_12345678901123456789012345678901234567890123"
    )]
    #[trace]
    fn should_include_bad_key_in_error_message_for_each_validation_branch_for_oauth_secret(
        #[case] bad_key: &str,
    ) {
        let err = RawOAuthClientSecret::try_from(bad_key.to_string()).unwrap_err();
        assert!(err.to_string().contains(bad_key));
    }
}

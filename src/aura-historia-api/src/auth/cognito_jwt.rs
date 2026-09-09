use crate::auth::core::{
    AuthError, AuthMethod, RequestMetadata, TokenAuthenticator, TransportPrincipal,
};
use jsonwebtokens::{Algorithm, AlgorithmID, Verifier, raw};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use user_service::ports::{CognitoIdentity, CognitoIssuer, CognitoSubject};
use user_service::use_cases::{
    ResolveCognitoUserError, ResolveCognitoUserRequest, ResolveCognitoUserUseCase,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CognitoJwtConfig {
    pub issuer: String,
    pub jwks_url: String,
    pub app_client_ids: HashSet<String>,
}

impl CognitoJwtConfig {
    pub fn new(
        issuer: impl Into<String>,
        jwks_url: impl Into<String>,
        app_client_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            jwks_url: jwks_url.into(),
            app_client_ids: app_client_ids.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonWebKeySet {
    pub keys: Vec<JsonWebKey>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonWebKey {
    pub kid: String,
    pub alg: Option<String>,
    pub n: String,
    pub e: String,
}

#[async_trait::async_trait]
pub trait JwksProvider: Send + Sync {
    async fn fetch_jwks(&self, jwks_url: &str) -> Result<JsonWebKeySet, AuthError>;
}

#[derive(Clone)]
pub struct ReqwestJwksProvider {
    client: reqwest::Client,
}

impl ReqwestJwksProvider {
    pub fn new(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl JwksProvider for ReqwestJwksProvider {
    async fn fetch_jwks(&self, jwks_url: &str) -> Result<JsonWebKeySet, AuthError> {
        self.client
            .get(jwks_url)
            .send()
            .await
            .map_err(|error| AuthError::JwksFetch(error.to_string()))?
            .error_for_status()
            .map_err(|error| AuthError::JwksFetch(error.to_string()))?
            .json()
            .await
            .map_err(|error| AuthError::JwksFetch(error.to_string()))
    }
}

const DEFAULT_JWKS_CACHE_TTL: Duration = Duration::from_secs(300);

struct JwksCache {
    algorithms: HashMap<String, Arc<Algorithm>>,
    refreshed_at: Option<Instant>,
}

pub struct CognitoJwtAuthenticator<P, R> {
    config: CognitoJwtConfig,
    identity_issuer: CognitoIssuer,
    provider: P,
    resolve_user: R,
    verifier: Verifier,
    cache_ttl: Duration,
    jwks_cache: Arc<RwLock<JwksCache>>,
    refresh_lock: Mutex<()>,
}

impl<P, R> CognitoJwtAuthenticator<P, R> {
    pub fn new(config: CognitoJwtConfig, provider: P, resolve_user: R) -> Result<Self, AuthError> {
        Self::with_cache_ttl(config, provider, resolve_user, DEFAULT_JWKS_CACHE_TTL)
    }

    fn with_cache_ttl(
        config: CognitoJwtConfig,
        provider: P,
        resolve_user: R,
        cache_ttl: Duration,
    ) -> Result<Self, AuthError> {
        let identity_issuer = CognitoIssuer::try_from(config.issuer.as_str())
            .map_err(|error| AuthError::Internal(error.to_string()))?;
        let verifier = Verifier::create()
            .issuer(config.issuer.clone())
            .build()
            .map_err(|error| AuthError::Internal(error.to_string()))?;
        Ok(Self {
            config,
            identity_issuer,
            provider,
            resolve_user,
            verifier,
            cache_ttl,
            jwks_cache: Arc::new(RwLock::new(JwksCache {
                algorithms: HashMap::new(),
                refreshed_at: None,
            })),
            refresh_lock: Mutex::new(()),
        })
    }
}

impl<P, R> CognitoJwtAuthenticator<P, R>
where
    P: JwksProvider,
{
    async fn algorithm_for_token(&self, token: &str) -> Result<Arc<Algorithm>, AuthError> {
        let header = raw::decode_header_only(token).map_err(|_| AuthError::MalformedCredentials)?;
        let kid = claim_string(&header, "kid")?;
        let (cached_algorithm, observed_refresh) = self.cached_algorithm(kid)?;

        if cached_algorithm.is_some() && self.is_cache_fresh(observed_refresh) {
            return cached_algorithm.ok_or(AuthError::JwksKeyNotFound);
        }

        let _refresh_guard = self.refresh_lock.lock().await;
        let (refreshed_algorithm, refreshed_at) = self.cached_algorithm(kid)?;
        if refreshed_at > observed_refresh {
            return refreshed_algorithm.ok_or(AuthError::JwksKeyNotFound);
        }

        match self.refresh_jwks().await {
            Ok(()) => self
                .cached_algorithm(kid)?
                .0
                .ok_or(AuthError::JwksKeyNotFound),
            Err(_) if cached_algorithm.is_some() => {
                cached_algorithm.ok_or(AuthError::JwksKeyNotFound)
            }
            Err(error) => Err(error),
        }
    }

    fn cached_algorithm(
        &self,
        kid: &str,
    ) -> Result<(Option<Arc<Algorithm>>, Option<Instant>), AuthError> {
        let cache = self
            .jwks_cache
            .read()
            .map_err(|error| AuthError::Internal(error.to_string()))?;
        Ok((cache.algorithms.get(kid).cloned(), cache.refreshed_at))
    }

    fn is_cache_fresh(&self, refreshed_at: Option<Instant>) -> bool {
        refreshed_at.is_some_and(|refreshed_at| refreshed_at.elapsed() < self.cache_ttl)
    }

    async fn refresh_jwks(&self) -> Result<(), AuthError> {
        let jwks = self
            .provider
            .fetch_jwks(&self.config.jwks_url)
            .await
            .map_err(|_| AuthError::TemporarilyUnavailable)?;
        let algorithms = jwks
            .keys
            .into_iter()
            .filter(|key| matches!(key.alg.as_deref(), None | Some("RS256")))
            .map(|key| {
                let mut algorithm =
                    Algorithm::new_rsa_n_e_b64_verifier(AlgorithmID::RS256, &key.n, &key.e)
                        .map_err(|error| AuthError::Internal(error.to_string()))?;
                algorithm.set_kid(&key.kid);
                Ok((key.kid, Arc::new(algorithm)))
            })
            .collect::<Result<HashMap<_, _>, AuthError>>()?;
        let mut cache = self
            .jwks_cache
            .write()
            .map_err(|error| AuthError::Internal(error.to_string()))?;
        cache.algorithms = algorithms;
        cache.refreshed_at = Some(Instant::now());
        Ok(())
    }
}

#[async_trait::async_trait]
impl<P, R> TokenAuthenticator for CognitoJwtAuthenticator<P, R>
where
    P: JwksProvider,
    R: ResolveCognitoUserUseCase,
{
    async fn authenticate(
        &self,
        bearer_token: &str,
        _metadata: &RequestMetadata,
    ) -> Result<TransportPrincipal, AuthError> {
        let algorithm = self.algorithm_for_token(bearer_token).await?;
        let claims = self
            .verifier
            .verify(bearer_token, &algorithm)
            .map_err(|_| AuthError::InvalidCredentials)?;

        verify_access_token(&claims, &self.config.app_client_ids)?;
        let subject = CognitoSubject::try_from(claim_string(&claims, "sub")?)
            .map_err(|_| AuthError::InvalidCredentials)?;
        let resolved = self
            .resolve_user
            .execute(ResolveCognitoUserRequest {
                identity: CognitoIdentity {
                    issuer: self.identity_issuer.clone(),
                    subject,
                },
            })
            .await
            .map_err(map_resolve_user_error)?;

        Ok(TransportPrincipal::User {
            user_id: resolved.user_id,
            auth_method: AuthMethod::CognitoJwt,
            capabilities: BTreeSet::new(),
        })
    }
}

fn map_resolve_user_error(error: ResolveCognitoUserError) -> AuthError {
    match error {
        ResolveCognitoUserError::NotFound => AuthError::InvalidCredentials,
        ResolveCognitoUserError::TemporarilyUnavailable { .. } => AuthError::TemporarilyUnavailable,
        ResolveCognitoUserError::InvalidPersistedState { .. }
        | ResolveCognitoUserError::Internal { .. } => AuthError::Internal(error.to_string()),
    }
}

fn verify_access_token(claims: &Value, app_client_ids: &HashSet<String>) -> Result<(), AuthError> {
    if claim_string(claims, "token_use")? != "access" {
        return Err(AuthError::InvalidCredentials);
    }
    if app_client_ids.contains(claim_string(claims, "client_id")?) {
        Ok(())
    } else {
        Err(AuthError::InvalidCredentials)
    }
}

fn claim_string<'a>(claims: &'a Value, claim: &'static str) -> Result<&'a str, AuthError> {
    claims
        .get(claim)
        .ok_or(AuthError::MissingClaim(claim))?
        .as_str()
        .ok_or(AuthError::InvalidClaimType(claim))
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::box_error;
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtokens as jwt;
    use openssl::rsa::Rsa;
    use serde_json::json;
    use std::sync::{Mutex, MutexGuard};
    use time::OffsetDateTime;
    use tokio::sync::Notify;
    use user_core::user_id::UserId;
    use user_service::use_cases::ResolveCognitoUserResult;

    const OPAQUE_SUBJECT: &str = "provider|opaque-user:42";

    #[derive(Clone)]
    struct FakeJwksProvider {
        result: Result<JsonWebKeySet, FakeJwksError>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[derive(Debug, Clone, Copy)]
    enum FakeJwksError {
        Fetch,
    }

    #[async_trait::async_trait]
    impl JwksProvider for FakeJwksProvider {
        async fn fetch_jwks(&self, jwks_url: &str) -> Result<JsonWebKeySet, AuthError> {
            lock(&self.calls).push(jwks_url.to_owned());
            self.result
                .clone()
                .map_err(|_| AuthError::JwksFetch("boom".to_owned()))
        }
    }

    #[derive(Clone)]
    struct SequencedJwksProvider {
        results: Arc<Mutex<Vec<Result<JsonWebKeySet, FakeJwksError>>>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl JwksProvider for SequencedJwksProvider {
        async fn fetch_jwks(&self, jwks_url: &str) -> Result<JsonWebKeySet, AuthError> {
            lock(&self.calls).push(jwks_url.to_owned());
            let mut results = lock(&self.results);
            if results.is_empty() {
                return Err(AuthError::JwksFetch("no configured response".to_owned()));
            }
            results
                .remove(0)
                .map_err(|_| AuthError::JwksFetch("boom".to_owned()))
        }
    }

    #[derive(Clone)]
    struct BlockingJwksProvider {
        jwks: JsonWebKeySet,
        calls: Arc<Mutex<Vec<String>>>,
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl JwksProvider for BlockingJwksProvider {
        async fn fetch_jwks(&self, jwks_url: &str) -> Result<JsonWebKeySet, AuthError> {
            lock(&self.calls).push(jwks_url.to_owned());
            self.entered.notify_one();
            self.release.notified().await;
            Ok(self.jwks.clone())
        }
    }

    #[derive(Clone, Copy)]
    enum ResolveOutcome {
        Found(UserId),
        NotFound,
        TemporarilyUnavailable,
        InvalidPersistedState,
        Internal,
    }

    type ResolveCalls = Arc<Mutex<Vec<ResolveCognitoUserRequest>>>;

    #[derive(Clone)]
    struct FakeResolveCognitoUserUseCase {
        outcome: ResolveOutcome,
        calls: ResolveCalls,
    }

    #[async_trait::async_trait]
    impl ResolveCognitoUserUseCase for FakeResolveCognitoUserUseCase {
        async fn execute(
            &self,
            request: ResolveCognitoUserRequest,
        ) -> Result<ResolveCognitoUserResult, ResolveCognitoUserError> {
            lock(&self.calls).push(request);
            match self.outcome {
                ResolveOutcome::Found(user_id) => Ok(ResolveCognitoUserResult { user_id }),
                ResolveOutcome::NotFound => Err(ResolveCognitoUserError::NotFound),
                ResolveOutcome::TemporarilyUnavailable => {
                    Err(ResolveCognitoUserError::TemporarilyUnavailable {
                        source: box_error(std::io::Error::other("temporary")),
                    })
                }
                ResolveOutcome::InvalidPersistedState => {
                    Err(ResolveCognitoUserError::InvalidPersistedState {
                        source: box_error(std::io::Error::other("invalid persisted mapping")),
                    })
                }
                ResolveOutcome::Internal => Err(ResolveCognitoUserError::Internal {
                    source: box_error(std::io::Error::other("internal")),
                }),
            }
        }
    }

    type TestAuthenticator =
        CognitoJwtAuthenticator<FakeJwksProvider, FakeResolveCognitoUserUseCase>;

    #[derive(Clone)]
    struct TestKey {
        kid: String,
        private_pem: Vec<u8>,
        n: String,
        e: String,
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn metadata() -> RequestMetadata {
        RequestMetadata::new("req-1", "corr-1")
    }

    fn test_key(kid: &str) -> Result<TestKey, Box<dyn std::error::Error>> {
        let rsa = Rsa::generate(2048)?;
        let private_pem = rsa.private_key_to_pem()?;
        Ok(TestKey {
            kid: kid.to_owned(),
            private_pem,
            n: URL_SAFE_NO_PAD.encode(rsa.n().to_vec()),
            e: URL_SAFE_NO_PAD.encode(rsa.e().to_vec()),
        })
    }

    fn jwk(key: &TestKey) -> JsonWebKey {
        JsonWebKey {
            kid: key.kid.clone(),
            alg: Some("RS256".to_owned()),
            n: key.n.clone(),
            e: key.e.clone(),
        }
    }

    fn authenticator(
        keys: Vec<JsonWebKey>,
        calls: Arc<Mutex<Vec<String>>>,
    ) -> Result<TestAuthenticator, AuthError> {
        authenticator_with_provider_result(Ok(JsonWebKeySet { keys }), calls)
    }

    fn authenticator_with_provider_result(
        result: Result<JsonWebKeySet, FakeJwksError>,
        calls: Arc<Mutex<Vec<String>>>,
    ) -> Result<TestAuthenticator, AuthError> {
        authenticator_with_resolution(
            result,
            calls,
            ResolveOutcome::Found(UserId::new()),
            Arc::new(Mutex::new(Vec::new())),
        )
    }

    fn authenticator_with_resolution(
        result: Result<JsonWebKeySet, FakeJwksError>,
        calls: Arc<Mutex<Vec<String>>>,
        outcome: ResolveOutcome,
        resolve_calls: ResolveCalls,
    ) -> Result<TestAuthenticator, AuthError> {
        CognitoJwtAuthenticator::new(
            CognitoJwtConfig::new(
                "https://issuer.example/pool",
                "https://issuer.example/pool/.well-known/jwks.json",
                ["audience-1"],
            ),
            FakeJwksProvider { result, calls },
            FakeResolveCognitoUserUseCase {
                outcome,
                calls: resolve_calls,
            },
        )
    }

    fn signed_jwt(key: &TestKey, claims: Value) -> Result<String, Box<dyn std::error::Error>> {
        let algorithm = Algorithm::new_rsa_pem_signer(AlgorithmID::RS256, &key.private_pem)?;
        let header = json!({ "alg": algorithm.name(), "kid": key.kid });
        Ok(jwt::encode(&header, &claims, &algorithm)?)
    }

    fn jwt_claims(subject: &str, exp_delta_seconds: i64, client_id: Value) -> Value {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        json!({
            "iss": "https://issuer.example/pool",
            "sub": subject,
            "token_use": "access",
            "client_id": client_id,
            "iat": now,
            "exp": now + exp_delta_seconds,
        })
    }

    #[test]
    fn should_build_cognito_jwt_config() {
        let config = CognitoJwtConfig::new("issuer", "jwks", ["aud-1", "aud-2"]);

        assert_eq!("issuer", config.issuer);
        assert_eq!("jwks", config.jwks_url);
        assert_eq!(
            HashSet::from(["aud-1".to_owned(), "aud-2".to_owned()]),
            config.app_client_ids
        );
    }

    #[test]
    fn should_create_reqwest_jwks_provider() {
        let _provider = ReqwestJwksProvider::new(reqwest::Client::new());
    }

    #[test]
    fn should_deserialize_jwks() -> Result<(), Box<dyn std::error::Error>> {
        let jwks: JsonWebKeySet = serde_json::from_value(json!({
            "keys": [{ "kid": "kid-1", "alg": "RS256", "n": "n", "e": "e" }]
        }))?;

        assert_eq!(1, jwks.keys.len());
        assert_eq!("kid-1", jwks.keys[0].kid);
        Ok(())
    }

    #[tokio::test]
    async fn should_authenticate_cognito_jwt_when_signature_claims_and_client_id_valid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let resolve_calls = Arc::new(Mutex::new(Vec::new()));
        let user_id = UserId::new();
        let authenticator = authenticator_with_resolution(
            Ok(JsonWebKeySet {
                keys: vec![jwk(&key)],
            }),
            calls.clone(),
            ResolveOutcome::Found(user_id),
            resolve_calls.clone(),
        )?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;

        let principal = authenticator.authenticate(&token, &metadata()).await?;

        assert!(matches!(
            principal,
            TransportPrincipal::User {
                user_id: actual,
                auth_method: AuthMethod::CognitoJwt,
                capabilities,
            } if actual == user_id && capabilities.is_empty()
        ));
        assert_eq!(
            vec!["https://issuer.example/pool/.well-known/jwks.json".to_owned()],
            *lock(&calls)
        );
        let resolve_calls = lock(&resolve_calls);
        assert_eq!(1, resolve_calls.len());
        assert_eq!(
            "https://issuer.example/pool",
            resolve_calls[0].identity.issuer.as_str()
        );
        assert_eq!(OPAQUE_SUBJECT, resolve_calls[0].identity.subject.as_str());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_id_token() -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1"));
        claims["token_use"] = json!("id");
        claims["aud"] = json!("audience-1");
        if let Some(claims) = claims.as_object_mut() {
            claims.remove("client_id");
        }
        let token = signed_jwt(&key, claims)?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_token_use_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1"));
        if let Some(claims) = claims.as_object_mut() {
            claims.remove("token_use");
        }
        let token = signed_jwt(&key, claims)?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::MissingClaim("token_use"))));
        Ok(())
    }

    #[tokio::test]
    async fn should_reuse_cached_jwks_when_kid_known() -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let authenticator = authenticator(vec![jwk(&key)], calls.clone())?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;

        let _ = authenticator.authenticate(&token, &metadata()).await?;
        let _ = authenticator.authenticate(&token, &metadata()).await?;

        assert_eq!(1, lock(&calls).len());
        Ok(())
    }

    #[tokio::test]
    async fn should_keep_cached_key_when_refresh_temporarily_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let authenticator = CognitoJwtAuthenticator::with_cache_ttl(
            CognitoJwtConfig::new(
                "https://issuer.example/pool",
                "https://issuer.example/pool/.well-known/jwks.json",
                ["audience-1"],
            ),
            SequencedJwksProvider {
                results: Arc::new(Mutex::new(vec![
                    Ok(JsonWebKeySet {
                        keys: vec![jwk(&key)],
                    }),
                    Err(FakeJwksError::Fetch),
                ])),
                calls: calls.clone(),
            },
            FakeResolveCognitoUserUseCase {
                outcome: ResolveOutcome::Found(UserId::new()),
                calls: Arc::new(Mutex::new(Vec::new())),
            },
            Duration::ZERO,
        )?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;

        let _ = authenticator.authenticate(&token, &metadata()).await?;
        let principal = authenticator.authenticate(&token, &metadata()).await?;

        assert!(matches!(principal, TransportPrincipal::User { .. }));
        assert_eq!(2, lock(&calls).len());
        Ok(())
    }

    #[tokio::test]
    async fn should_refresh_jwks_once_for_concurrent_unknown_kid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let authenticator = Arc::new(CognitoJwtAuthenticator::new(
            CognitoJwtConfig::new(
                "https://issuer.example/pool",
                "https://issuer.example/pool/.well-known/jwks.json",
                ["audience-1"],
            ),
            BlockingJwksProvider {
                jwks: JsonWebKeySet {
                    keys: vec![jwk(&key)],
                },
                calls: calls.clone(),
                entered: entered.clone(),
                release: release.clone(),
            },
            FakeResolveCognitoUserUseCase {
                outcome: ResolveOutcome::Found(UserId::new()),
                calls: Arc::new(Mutex::new(Vec::new())),
            },
        )?);
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;
        let first_authenticator = authenticator.clone();
        let first_token = token.clone();
        let first = tokio::spawn(async move {
            first_authenticator
                .authenticate(&first_token, &metadata())
                .await
        });
        entered.notified().await;

        let mut pending = Vec::new();
        for _ in 0..4 {
            let authenticator = authenticator.clone();
            let token = token.clone();
            pending.push(tokio::spawn(async move {
                authenticator.authenticate(&token, &metadata()).await
            }));
        }
        release.notify_waiters();

        assert!(matches!(first.await??, TransportPrincipal::User { .. }));
        for pending_authentication in pending {
            assert!(matches!(
                pending_authentication.await??,
                TransportPrincipal::User { .. }
            ));
        }
        assert_eq!(1, lock(&calls).len());
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_expired() -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, -60, json!("audience-1")))?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_issuer_wrong() -> Result<(), Box<dyn std::error::Error>>
    {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1"));
        claims["iss"] = json!("https://evil.example/pool");
        let token = signed_jwt(&key, claims)?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_client_id_wrong()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(
            &key,
            jwt_claims(OPAQUE_SUBJECT, 3_600, json!("other-audience")),
        )?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_client_id_type_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!(123)))?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(
            result,
            Err(AuthError::InvalidClaimType("client_id"))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_client_id_array_value_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!([123])))?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(
            result,
            Err(AuthError::InvalidClaimType("client_id"))
        ));
        Ok(())
    }

    #[test]
    fn should_reject_client_id_claim_when_type_invalid() {
        let result = verify_access_token(
            &json!({ "token_use": "access", "client_id": 123 }),
            &HashSet::new(),
        );

        assert!(matches!(
            result,
            Err(AuthError::InvalidClaimType("client_id"))
        ));
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_sub_missing() -> Result<(), Box<dyn std::error::Error>>
    {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1"));
        if let Some(claims) = claims.as_object_mut() {
            claims.remove("sub");
        }
        let token = signed_jwt(&key, claims)?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::MissingClaim("sub"))));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_sub_type_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let mut claims = jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1"));
        claims["sub"] = json!(123);
        let token = signed_jwt(&key, claims)?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_opaque_identity_unknown()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let resolve_calls = Arc::new(Mutex::new(Vec::new()));
        let authenticator = authenticator_with_resolution(
            Ok(JsonWebKeySet {
                keys: vec![jwk(&key)],
            }),
            Arc::new(Mutex::new(Vec::new())),
            ResolveOutcome::NotFound,
            resolve_calls.clone(),
        )?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidCredentials)));
        let resolve_calls = lock(&resolve_calls);
        assert_eq!(1, resolve_calls.len());
        assert_eq!(OPAQUE_SUBJECT, resolve_calls[0].identity.subject.as_str());
        Ok(())
    }

    #[tokio::test]
    async fn should_map_cognito_identity_resolution_service_errors()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;

        for (outcome, temporary) in [
            (ResolveOutcome::TemporarilyUnavailable, true),
            (ResolveOutcome::InvalidPersistedState, false),
            (ResolveOutcome::Internal, false),
        ] {
            let authenticator = authenticator_with_resolution(
                Ok(JsonWebKeySet {
                    keys: vec![jwk(&key)],
                }),
                Arc::new(Mutex::new(Vec::new())),
                outcome,
                Arc::new(Mutex::new(Vec::new())),
            )?;

            let result = authenticator.authenticate(&token, &metadata()).await;

            if temporary {
                assert!(matches!(result, Err(AuthError::TemporarilyUnavailable)));
            } else {
                assert!(matches!(result, Err(AuthError::Internal(_))));
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_token_malformed() -> Result<(), AuthError> {
        let authenticator = authenticator(Vec::new(), Arc::new(Mutex::new(Vec::new())))?;

        let result = authenticator.authenticate("not-a-jwt", &metadata()).await;

        assert!(matches!(result, Err(AuthError::MalformedCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_kid_missing() -> Result<(), Box<dyn std::error::Error>>
    {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let algorithm = Algorithm::new_rsa_pem_signer(AlgorithmID::RS256, &key.private_pem)?;
        let header = json!({ "alg": algorithm.name() });
        let token = jwt::encode(
            &header,
            &jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")),
            &algorithm,
        )?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::MissingClaim("kid"))));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_kid_type_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator(vec![jwk(&key)], Arc::new(Mutex::new(Vec::new())))?;
        let algorithm = Algorithm::new_rsa_pem_signer(AlgorithmID::RS256, &key.private_pem)?;
        let header = json!({ "alg": algorithm.name(), "kid": 123 });
        let token = jwt::encode(
            &header,
            &jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")),
            &algorithm,
        )?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::InvalidClaimType("kid"))));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_jwks_fetch_fails() -> Result<(), AuthError> {
        let authenticator = authenticator_with_provider_result(
            Err(FakeJwksError::Fetch),
            Arc::new(Mutex::new(Vec::new())),
        )?;

        let result = authenticator
            .authenticate("header.claims.signature", &metadata())
            .await;

        assert!(matches!(result, Err(AuthError::MalformedCredentials)));
        Ok(())
    }

    #[tokio::test]
    async fn should_reject_cognito_jwt_when_jwks_missing_kid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let other_key = test_key("kid-2")?;
        let authenticator = authenticator(vec![jwk(&other_key)], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::JwksKeyNotFound)));
        Ok(())
    }

    #[tokio::test]
    async fn should_ignore_jwks_key_when_alg_not_rs256() -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let mut key_record = jwk(&key);
        key_record.alg = Some("RS384".to_owned());
        let authenticator = authenticator(vec![key_record], Arc::new(Mutex::new(Vec::new())))?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::JwksKeyNotFound)));
        Ok(())
    }

    #[tokio::test]
    async fn should_map_jwks_provider_failure_when_header_valid()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = test_key("kid-1")?;
        let authenticator = authenticator_with_provider_result(
            Err(FakeJwksError::Fetch),
            Arc::new(Mutex::new(Vec::new())),
        )?;
        let token = signed_jwt(&key, jwt_claims(OPAQUE_SUBJECT, 3_600, json!("audience-1")))?;

        let result = authenticator.authenticate(&token, &metadata()).await;

        assert!(matches!(result, Err(AuthError::TemporarilyUnavailable)));
        Ok(())
    }
}

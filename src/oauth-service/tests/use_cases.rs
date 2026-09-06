use application::operation_context::{
    CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
};
use application::pagination::Cursor;
use application::transaction::{Transaction, TransactionError, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use domain_primitives::versioned::Versioned;
use oauth_core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
    OAuthCodeVerifier, RehydratedAuthorizationCodeState,
};
use oauth_core::client::{
    OAuthClient, OAuthClientName, OAuthRedirectUris, RehydratedOAuthClientState,
};
use oauth_core::third_party_exchange_code::{
    RehydratedThirdPartyExchangeCodeGrantState, ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant,
};
use oauth_service::error::OAuthServiceError;
use oauth_service::ports::*;
use oauth_service::use_cases::{
    AuthorizeHandler, AuthorizeRequest, AuthorizeUseCase, CreateOAuthClientCommand,
    CreateOAuthClientHandler, CreateOAuthClientUseCase, DeleteOAuthClientHandler,
    DeleteOAuthClientUseCase, GetOAuthClientHandler, GetOAuthClientUseCase, IntrospectTokenHandler,
    IntrospectTokenRequest, IntrospectTokenUseCase, ListOAuthClientsHandler,
    ListOAuthClientsRequest, ListOAuthClientsResult, ListOAuthClientsUseCase, OAuthGrantType,
    OAuthResponseType, OAuthState, OAuthTokenType, RevokeTokenHandler, RevokeTokenRequest,
    RevokeTokenUseCase, TokenByAuthorizationCodeHandler, TokenByAuthorizationCodeRequest,
    TokenByAuthorizationCodeUseCase, TokenByThirdPartyCodeHandler, TokenByThirdPartyCodeUseCase,
    UpdateOAuthClientCommand, UpdateOAuthClientHandler, UpdateOAuthClientUseCase,
};
use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, HashedRawOAuthClientSecret,
    NewAccessToken, RawAccessToken, RawOAuthClientSecret, Scope,
};
use user_core::user_id::UserId;
use user_service::ports::{
    AccessTokenAuthentication, AccessTokenAuthenticationReadError, AccessTokenAuthenticationReader,
    AccessTokenRepository, AccessTokenRepositoryError, AccessTokenRepositoryFactory,
    AccessTokenStorageVersion, VersionedAccessToken,
};
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult, CheckUserAdminUseCase,
};

#[derive(Clone, Default)]
struct State {
    client: Option<OAuthClient>,

    code: Option<AuthorizationCode>,
    exchange: Option<ThirdPartyExchangeCodeGrant>,
    issued: Option<AccessToken>,
    deleted_raw: usize,
    client_updates: usize,
    details_reads: usize,
    list_reads: usize,
    transaction_begins: usize,
    transaction_commits: usize,
    fail_access_token_insert: bool,
    fail_exchange_insert: bool,
}

#[derive(Clone, Default)]
struct FakePorts(Arc<Mutex<State>>);

struct FakeTransaction {
    ports: FakePorts,
    staged: State,
}

#[derive(Clone)]
struct FakeUnitOfWork(FakePorts);

struct TransactionalFakePorts<'tx> {
    transaction: &'tx mut FakeTransaction,
}

impl<'tx> TransactionalFakePorts<'tx> {
    fn state(&mut self) -> &mut State {
        &mut self.transaction.staged
    }
}

fn persisted_client(
    value: OAuthClient,
    version: OAuthClientStorageVersion,
) -> PersistedOAuthClient {
    PersistedOAuthClient {
        value,
        version,
        created: OffsetDateTime::UNIX_EPOCH,
        updated: OffsetDateTime::UNIX_EPOCH,
    }
}

#[async_trait::async_trait]
impl Transaction for FakeTransaction {
    async fn commit(self) -> Result<(), TransactionError> {
        let mut state = lock(&self.ports.0);
        let mut staged = self.staged;
        staged.transaction_commits = state.transaction_commits + 1;
        *state = staged;
        Ok(())
    }
}

#[async_trait::async_trait]
impl UnitOfWork for FakeUnitOfWork {
    type Tx = FakeTransaction;

    async fn begin(&self) -> Result<Self::Tx, TransactionError> {
        let mut state = lock(&self.0.0);
        state.transaction_begins += 1;
        Ok(FakeTransaction {
            ports: self.0.clone(),
            staged: state.clone(),
        })
    }
}

impl OAuthClientRepositoryFactory<FakeTransaction> for FakePorts {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut FakeTransaction,
    ) -> impl OAuthClientRepository + 'tx {
        TransactionalFakePorts { transaction: tx }
    }
}

impl AuthorizationCodeRepositoryFactory<FakeTransaction> for FakePorts {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut FakeTransaction,
    ) -> impl AuthorizationCodeRepository + 'tx {
        TransactionalFakePorts { transaction: tx }
    }
}

impl ThirdPartyExchangeCodeRepositoryFactory<FakeTransaction> for FakePorts {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut FakeTransaction,
    ) -> impl ThirdPartyExchangeCodeRepository + 'tx {
        TransactionalFakePorts { transaction: tx }
    }
}

impl AccessTokenRepositoryFactory<FakeTransaction> for FakePorts {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut FakeTransaction,
    ) -> impl AccessTokenRepository + 'tx {
        TransactionalFakePorts { transaction: tx }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: RequestId::new("req"),
        correlation_id: CorrelationId::new("corr"),
    }
}

fn ctx() -> OperationContext {
    context(Principal::DelegatedUser {
        user_id: UserId::new(),
        capabilities: BTreeSet::from([
            CredentialCapability::AccessTokensRead,
            CredentialCapability::AccessTokensWrite,
            CredentialCapability::ProductListingsWrite,
        ]),
    })
}

fn create_command() -> CreateOAuthClientCommand {
    CreateOAuthClientCommand {
        name: OAuthClientName::from("A"),
        redirect_uris: HashSet::from([url("https://client.example/callback")]),
        tos_uri: url("https://client.example/tos"),
        policy_uri: url("https://client.example/policy"),
        client_uri: url("https://client.example"),
        logo_uri: url("https://client.example/logo.png"),
        scopes: HashSet::from([Scope::ProductListingsWrite]),
    }
}

fn url(value: &str) -> url::Url {
    match url::Url::parse(value) {
        Ok(url) => url,
        Err(error) => panic!("test URL must be valid: {error}"),
    }
}

fn authorization_code(client_id: OAuthClientId, expires: OffsetDateTime) -> AuthorizationCode {
    AuthorizationCode::create(RehydratedAuthorizationCodeState {
        code: OAuthAuthorizationCode::new(),
        client_id,
        user_id: UserId::new(),
        redirect_uri: url("https://client.example/callback"),
        scopes: HashSet::from([Scope::ProductListingsWrite]),
        code_challenge: OAuthCodeChallenge::from("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
        code_challenge_method: CodeChallengeMethod::S256,
        expires,
    })
}

fn client_with_secret(raw: &RawOAuthClientSecret) -> OAuthClient {
    OAuthClient::create(RehydratedOAuthClientState {
        client_id: OAuthClientId::new(),
        hashed_client_secret: HashedRawOAuthClientSecret::from(raw.clone()),
        name: OAuthClientName::from("Test Client"),
        redirect_uris: OAuthRedirectUris::try_from(HashSet::from([url(
            "https://client.example/callback",
        )]))
        .unwrap_or_else(|error| panic!("test redirect URI must be valid: {error}")),
        tos_uri: url("https://client.example/tos"),
        policy_uri: url("https://client.example/policy"),
        client_uri: url("https://client.example"),
        logo_uri: url("https://client.example/logo.png"),
        scopes: HashSet::from([Scope::ProductListingsWrite]),
    })
}

fn authorize_request(client_id: OAuthClientId, scope: HashSet<Scope>) -> AuthorizeRequest {
    AuthorizeRequest {
        response_type: OAuthResponseType::Code,
        client_id,
        redirect_uri: url("https://client.example/callback"),
        scope,
        state: None,
        code_challenge: OAuthCodeChallenge::from("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
        code_challenge_method: CodeChallengeMethod::S256,
    }
}

async fn redeem_authorization_code(
    ports: FakePorts,
    code: AuthorizationCode,
    client_id: OAuthClientId,
    client_secret: RawOAuthClientSecret,
    redirect_uri: url::Url,
    code_verifier: &str,
) -> (OAuthServiceError, State) {
    let code_value = code.code();
    let error = TokenByAuthorizationCodeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
    )
    .execute(TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code: code_value,
        redirect_uri,
        client_id,
        client_secret,
        code_verifier: OAuthCodeVerifier::from(code_verifier),
    })
    .await
    .unwrap_err();
    let state = lock(&ports.0).clone();
    (error, state)
}

fn client_view(client: OAuthClient) -> OAuthClientView {
    OAuthClientView {
        client_id: client.client_id(),
        name: client.name().clone(),
        redirect_uris: client.redirect_uris().as_set().clone(),
        tos_uri: client.tos_uri().clone(),
        policy_uri: client.policy_uri().clone(),
        client_uri: client.client_uri().clone(),
        logo_uri: client.logo_uri().clone(),
        scopes: client.scopes().clone(),
        created: OffsetDateTime::UNIX_EPOCH,
        updated: OffsetDateTime::UNIX_EPOCH,
    }
}

#[async_trait::async_trait]
impl OAuthClientAuthenticationReader for FakePorts {
    async fn find_by_id(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientAuthentication>, OAuthClientReadError> {
        Ok(lock(&self.0)
            .client
            .as_ref()
            .filter(|client| client.client_id() == *client_id)
            .map(|client| OAuthClientAuthentication {
                hashed_client_secret: client.hashed_client_secret().clone(),
            }))
    }
}

#[async_trait::async_trait]
impl OAuthClientDetailsReader for FakePorts {
    async fn find(
        &self,
        client_id: &OAuthClientId,
    ) -> Result<Option<OAuthClientView>, OAuthClientReadError> {
        let mut state = lock(&self.0);
        state.details_reads += 1;
        Ok(state
            .client
            .clone()
            .filter(|client| client.client_id() == *client_id)
            .map(client_view))
    }
}

#[async_trait::async_trait]
impl OAuthClientListReader for FakePorts {
    async fn search(
        &self,
        request: &ListOAuthClientsRequest,
    ) -> Result<ListOAuthClientsResult, OAuthClientReadError> {
        let mut state = lock(&self.0);
        state.list_reads += 1;
        Ok(ListOAuthClientsResult {
            items: state.client.clone().into_iter().map(client_view).collect(),
            cursor: request.cursor.unwrap_or_default(),
            total: None,
        })
    }
}

struct FakeCheckUserAdmin {
    admin_user_id: Option<UserId>,
}

impl FakeCheckUserAdmin {
    fn allow_all() -> Self {
        Self {
            admin_user_id: None,
        }
    }

    fn allow_user(admin_user_id: UserId) -> Self {
        Self {
            admin_user_id: Some(admin_user_id),
        }
    }
}

#[async_trait::async_trait]
impl CheckUserAdminUseCase for FakeCheckUserAdmin {
    async fn execute(
        &self,
        context: &OperationContext,
        _request: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        let allowed = match self.admin_user_id {
            None => true,
            Some(admin_user_id) => match &context.principal {
                Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => {
                    *user_id == admin_user_id
                }
                _ => false,
            },
        };
        if allowed {
            Ok(CheckUserAdminResult)
        } else {
            Err(CheckUserAdminError::Forbidden)
        }
    }
}

#[async_trait::async_trait]
impl OAuthClientRepository for TransactionalFakePorts<'_> {
    async fn find_by_id(
        &mut self,
        client_id: OAuthClientId,
    ) -> Result<Option<VersionedOAuthClient>, OAuthClientRepositoryError> {
        Ok(self
            .state()
            .client
            .clone()
            .filter(|client| client.client_id() == client_id)
            .map(|client| persisted_client(client, OAuthClientStorageVersion::INITIAL)))
    }

    async fn insert(
        &mut self,
        client: &OAuthClient,
    ) -> Result<VersionedOAuthClient, OAuthClientRepositoryError> {
        self.state().client = Some(client.clone());
        Ok(persisted_client(
            client.clone(),
            OAuthClientStorageVersion::INITIAL,
        ))
    }

    async fn update(
        &mut self,
        client: &OAuthClient,
        expected_version: OAuthClientStorageVersion,
    ) -> Result<VersionedOAuthClient, OAuthClientRepositoryError> {
        if expected_version != OAuthClientStorageVersion::INITIAL {
            return Err(OAuthClientRepositoryError::ConcurrencyConflict);
        }
        let state = self.state();
        let Some(stored) = state.client.as_mut() else {
            return Err(OAuthClientRepositoryError::ConcurrencyConflict);
        };
        if stored.client_id() != client.client_id() {
            return Err(OAuthClientRepositoryError::ConcurrencyConflict);
        }
        *stored = client.clone();
        let updated = stored.clone();
        state.client_updates += 1;
        Ok(persisted_client(
            updated,
            OAuthClientStorageVersion::try_from(2_i64).map_err(|source| {
                OAuthClientRepositoryError::InvalidPersistedState {
                    source: application::error::box_error(source),
                }
            })?,
        ))
    }

    async fn delete_by_id(
        &mut self,
        client_id: OAuthClientId,
    ) -> Result<bool, OAuthClientRepositoryError> {
        let state = self.state();
        let deleted = state
            .client
            .as_ref()
            .is_some_and(|client| client.client_id() == client_id);
        if deleted {
            state.client = None;
        }
        Ok(deleted)
    }
}

#[async_trait::async_trait]
impl AuthorizationCodeRepository for TransactionalFakePorts<'_> {
    async fn insert(&mut self, code: AuthorizationCode) -> Result<(), OAuthCodeRepositoryError> {
        self.state().code = Some(code);
        Ok(())
    }

    async fn consume_by_code(
        &mut self,
        code: &OAuthAuthorizationCode,
    ) -> Result<Option<AuthorizationCode>, OAuthCodeRepositoryError> {
        let state = self.state();
        if state
            .code
            .as_ref()
            .is_some_and(|stored| stored.code() == *code)
        {
            Ok(state.code.take())
        } else {
            Ok(None)
        }
    }
}

#[async_trait::async_trait]
impl ThirdPartyExchangeCodeRepository for TransactionalFakePorts<'_> {
    async fn insert(
        &mut self,
        grant: ThirdPartyExchangeCodeGrant,
    ) -> Result<(), OAuthCodeRepositoryError> {
        if self.state().fail_exchange_insert {
            return Err(OAuthCodeRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other("exchange failed")),
            });
        }
        self.state().exchange = Some(grant);
        Ok(())
    }

    async fn consume_by_code(
        &mut self,
        code: &ThirdPartyExchangeCode,
    ) -> Result<Option<ThirdPartyExchangeCodeGrant>, OAuthCodeRepositoryError> {
        let state = self.state();
        if state
            .exchange
            .as_ref()
            .is_some_and(|stored| stored.code() == *code)
        {
            Ok(state.exchange.take())
        } else {
            Ok(None)
        }
    }
}

#[async_trait::async_trait]
impl AccessTokenAuthenticationReader for FakePorts {
    async fn find_authentication_by_hashed_token(
        &self,
        hashed_token: &user_core::access_token::HashedRawAccessToken,
    ) -> Result<Option<AccessTokenAuthentication>, AccessTokenAuthenticationReadError> {
        Ok(lock(&self.0)
            .issued
            .as_ref()
            .filter(|token| token.hashed_token() == hashed_token)
            .map(|token| AccessTokenAuthentication {
                access_token_id: token.id(),
                user_id: token.user_id(),
                scopes: token.scopes().clone(),
                origin: token.origin().clone(),
                expires: token.expires(),
            }))
    }
}

#[async_trait::async_trait]
impl AccessTokenRepository for TransactionalFakePorts<'_> {
    async fn find_by_id(
        &mut self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
        Ok(self
            .state()
            .issued
            .clone()
            .filter(|token| token.user_id() == user_id && token.id() == access_token_id)
            .map(|token| Versioned::new(token, AccessTokenStorageVersion::INITIAL)))
    }

    async fn find_by_hashed_token(
        &mut self,
        hashed_token: &user_core::access_token::HashedRawAccessToken,
    ) -> Result<Option<VersionedAccessToken>, AccessTokenRepositoryError> {
        Ok(self
            .state()
            .issued
            .clone()
            .filter(|token| token.hashed_token() == hashed_token)
            .map(|token| Versioned::new(token, AccessTokenStorageVersion::INITIAL)))
    }

    async fn insert(
        &mut self,
        access_token: &AccessToken,
    ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
        if self.state().fail_access_token_insert {
            return Err(AccessTokenRepositoryError::Internal {
                source: application::error::box_error(std::io::Error::other(
                    "access token insert failed",
                )),
            });
        }
        self.state().issued = Some(access_token.clone());
        Ok(Versioned::new(
            access_token.clone(),
            AccessTokenStorageVersion::INITIAL,
        ))
    }

    async fn update(
        &mut self,
        access_token: &AccessToken,
        _expected_version: AccessTokenStorageVersion,
    ) -> Result<VersionedAccessToken, AccessTokenRepositoryError> {
        self.state().issued = Some(access_token.clone());
        Ok(Versioned::new(
            access_token.clone(),
            AccessTokenStorageVersion::INITIAL,
        ))
    }

    async fn delete_by_id(
        &mut self,
        user_id: UserId,
        access_token_id: AccessTokenId,
    ) -> Result<bool, AccessTokenRepositoryError> {
        let state = self.state();
        let deleted = state
            .issued
            .as_ref()
            .is_some_and(|token| token.user_id() == user_id && token.id() == access_token_id);
        if deleted {
            state.issued = None;
            state.deleted_raw += 1;
        }
        Ok(deleted)
    }
}

#[tokio::test]
async fn should_cover_client_crud_use_cases() {
    let ports = FakePorts::default();
    let create = CreateOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        FakeCheckUserAdmin::allow_all(),
    );
    let result = create.execute(&ctx(), create_command()).await.unwrap();
    let client_id = result.client.client_id;
    assert!(
        result.raw_client_secret.check(
            lock(&ports.0)
                .client
                .as_ref()
                .map(OAuthClient::hashed_client_secret)
                .unwrap_or_else(|| panic!("created client must be stored")),
        )
    );
    assert_eq!(
        1,
        ListOAuthClientsHandler::new(ports.clone(), FakeCheckUserAdmin::allow_all())
            .execute(&ctx(), ListOAuthClientsRequest::default())
            .await
            .unwrap()
            .items
            .len()
    );
    assert_eq!(
        client_id,
        GetOAuthClientHandler::new(ports.clone(), FakeCheckUserAdmin::allow_all())
            .execute(&ctx(), &client_id)
            .await
            .unwrap()
            .client_id
    );
    let updated = UpdateOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        FakeCheckUserAdmin::allow_all(),
    )
    .execute(
        &ctx(),
        &client_id,
        UpdateOAuthClientCommand {
            name: Some(OAuthClientName::from("B")),
            redirect_uris: None,
            tos_uri: None,
            policy_uri: None,
            client_uri: None,
            logo_uri: None,
            scopes: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(OAuthClientName::from("B"), updated.name);
    DeleteOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        FakeCheckUserAdmin::allow_all(),
    )
    .execute(&ctx(), &client_id)
    .await
    .unwrap();
    assert!(
        ListOAuthClientsHandler::new(ports.clone(), FakeCheckUserAdmin::allow_all())
            .execute(&ctx(), ListOAuthClientsRequest::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let state = lock(&ports.0);
    assert_eq!(1, state.client_updates);
    assert_eq!(3, state.transaction_begins);
    assert_eq!(3, state.transaction_commits);
}

#[tokio::test]
async fn should_require_admin_role_and_delegated_write_for_oauth_client_creation() {
    let admin_user_id = UserId::new();
    let ports = FakePorts::default();
    let create = CreateOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        FakeCheckUserAdmin::allow_user(admin_user_id),
    );

    let direct_non_admin = context(Principal::User(UserId::new()));
    assert!(matches!(
        create.execute(&direct_non_admin, create_command()).await,
        Err(OAuthServiceError::Forbidden)
    ));

    let delegated_without_write = context(Principal::DelegatedUser {
        user_id: admin_user_id,
        capabilities: BTreeSet::new(),
    });
    assert!(matches!(
        create
            .execute(&delegated_without_write, create_command())
            .await,
        Err(OAuthServiceError::Forbidden)
    ));

    let direct_admin = context(Principal::User(admin_user_id));
    assert!(
        create
            .execute(&direct_admin, create_command())
            .await
            .is_ok()
    );

    let delegated_admin = context(Principal::DelegatedUser {
        user_id: admin_user_id,
        capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
    });
    assert!(
        create
            .execute(&delegated_admin, create_command())
            .await
            .is_ok()
    );
    assert_eq!(2, lock(&ports.0).transaction_begins);
    assert_eq!(2, lock(&ports.0).transaction_commits);
}

#[tokio::test]
async fn should_require_admin_role_and_delegated_write_for_oauth_client_update() {
    let ports = FakePorts::default();
    let client = client_with_secret(&RawOAuthClientSecret::new());
    let client_id = client.client_id();
    lock(&ports.0).client = Some(client);
    let admin_user_id = UserId::new();
    let update = UpdateOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        FakeCheckUserAdmin::allow_user(admin_user_id),
    );
    let command = |name: &str| UpdateOAuthClientCommand {
        name: Some(OAuthClientName::from(name)),
        redirect_uris: None,
        tos_uri: None,
        policy_uri: None,
        client_uri: None,
        logo_uri: None,
        scopes: None,
    };

    let anonymous = context(Principal::Anonymous);
    assert!(matches!(
        update
            .execute(&anonymous, &client_id, command("Anonymous"))
            .await,
        Err(OAuthServiceError::AuthenticatedActorRequired)
    ));

    let direct_non_admin = context(Principal::User(UserId::new()));
    assert!(matches!(
        update
            .execute(&direct_non_admin, &client_id, command("Non-admin"))
            .await,
        Err(OAuthServiceError::Forbidden)
    ));

    let delegated_without_write = context(Principal::DelegatedUser {
        user_id: admin_user_id,
        capabilities: BTreeSet::new(),
    });
    assert!(matches!(
        update
            .execute(
                &delegated_without_write,
                &client_id,
                command("Missing capability")
            )
            .await,
        Err(OAuthServiceError::Forbidden)
    ));

    let direct_admin = context(Principal::User(admin_user_id));
    assert!(
        update
            .execute(&direct_admin, &client_id, command("Direct admin"))
            .await
            .is_ok()
    );

    let delegated_admin = context(Principal::DelegatedUser {
        user_id: admin_user_id,
        capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
    });
    assert!(
        update
            .execute(&delegated_admin, &client_id, command("Delegated admin"))
            .await
            .is_ok()
    );

    let state = lock(&ports.0);
    assert_eq!(2, state.client_updates);
    assert_eq!(2, state.transaction_begins);
    assert_eq!(2, state.transaction_commits);
}

#[tokio::test]
async fn should_require_admin_role_and_delegated_write_for_oauth_client_deletion() {
    let ports = FakePorts::default();
    let client = client_with_secret(&RawOAuthClientSecret::new());
    let client_id = client.client_id();
    lock(&ports.0).client = Some(client);
    let admin_user_id = UserId::new();
    let delete = DeleteOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        FakeCheckUserAdmin::allow_user(admin_user_id),
    );

    let anonymous = context(Principal::Anonymous);
    assert!(matches!(
        delete.execute(&anonymous, &client_id).await,
        Err(OAuthServiceError::AuthenticatedActorRequired)
    ));

    let direct_non_admin = context(Principal::User(UserId::new()));
    assert!(matches!(
        delete.execute(&direct_non_admin, &client_id).await,
        Err(OAuthServiceError::Forbidden)
    ));

    let delegated_without_write = context(Principal::DelegatedUser {
        user_id: admin_user_id,
        capabilities: BTreeSet::new(),
    });
    assert!(matches!(
        delete.execute(&delegated_without_write, &client_id).await,
        Err(OAuthServiceError::Forbidden)
    ));

    let direct_admin = context(Principal::User(admin_user_id));
    assert!(delete.execute(&direct_admin, &client_id).await.is_ok());

    let second_client = client_with_secret(&RawOAuthClientSecret::new());
    let second_client_id = second_client.client_id();
    lock(&ports.0).client = Some(second_client);
    let delegated_admin = context(Principal::DelegatedUser {
        user_id: admin_user_id,
        capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
    });
    assert!(
        delete
            .execute(&delegated_admin, &second_client_id)
            .await
            .is_ok()
    );

    let state = lock(&ports.0);
    assert_eq!(2, state.transaction_begins);
    assert_eq!(2, state.transaction_commits);
}

#[tokio::test]
async fn should_authorize_oauth_client_get_and_admin_list_in_the_service() {
    let ports = FakePorts::default();
    let client = client_with_secret(&RawOAuthClientSecret::new());
    let client_id = client.client_id();
    lock(&ports.0).client = Some(client);
    let admin_user_id = UserId::new();
    let get =
        GetOAuthClientHandler::new(ports.clone(), FakeCheckUserAdmin::allow_user(admin_user_id));
    let list =
        ListOAuthClientsHandler::new(ports.clone(), FakeCheckUserAdmin::allow_user(admin_user_id));

    let anonymous = context(Principal::Anonymous);
    assert!(matches!(
        get.execute(&anonymous, &client_id).await,
        Err(OAuthServiceError::AuthenticatedActorRequired)
    ));
    assert!(matches!(
        list.execute(&anonymous, ListOAuthClientsRequest::default())
            .await,
        Err(OAuthServiceError::AuthenticatedActorRequired)
    ));

    let delegated_without_read = context(Principal::DelegatedUser {
        user_id: UserId::new(),
        capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
    });
    assert!(matches!(
        get.execute(&delegated_without_read, &client_id).await,
        Err(OAuthServiceError::Forbidden)
    ));
    assert!(matches!(
        list.execute(&delegated_without_read, ListOAuthClientsRequest::default())
            .await,
        Err(OAuthServiceError::Forbidden)
    ));
    assert_eq!(0, lock(&ports.0).details_reads);
    assert_eq!(0, lock(&ports.0).list_reads);

    let delegated_non_admin = context(Principal::DelegatedUser {
        user_id: UserId::new(),
        capabilities: BTreeSet::from([CredentialCapability::AccessTokensRead]),
    });
    assert!(matches!(
        get.execute(&delegated_non_admin, &client_id).await,
        Err(OAuthServiceError::Forbidden)
    ));
    assert!(matches!(
        list.execute(&delegated_non_admin, ListOAuthClientsRequest::default())
            .await,
        Err(OAuthServiceError::Forbidden)
    ));

    let direct_non_admin = context(Principal::User(UserId::new()));
    assert!(matches!(
        get.execute(&direct_non_admin, &client_id).await,
        Err(OAuthServiceError::Forbidden)
    ));
    assert!(matches!(
        list.execute(&direct_non_admin, ListOAuthClientsRequest::default())
            .await,
        Err(OAuthServiceError::Forbidden)
    ));

    let delegated_admin = context(Principal::DelegatedUser {
        user_id: admin_user_id,
        capabilities: BTreeSet::from([CredentialCapability::AccessTokensRead]),
    });
    assert!(get.execute(&delegated_admin, &client_id).await.is_ok());
    let result = list
        .execute(
            &delegated_admin,
            ListOAuthClientsRequest {
                cursor: Some(Cursor {
                    size: 1_000,
                    search_after: None,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(100, result.cursor.size);

    let direct_admin = context(Principal::User(admin_user_id));
    assert!(get.execute(&direct_admin, &client_id).await.is_ok());
    assert!(
        list.execute(&direct_admin, ListOAuthClientsRequest::default())
            .await
            .is_ok()
    );
    assert_eq!(2, lock(&ports.0).details_reads);
    assert_eq!(2, lock(&ports.0).list_reads);
}

#[tokio::test]
async fn should_skip_no_op_and_optimistically_update_changed_client_metadata() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    lock(&ports.0).client = Some(client.clone());

    let no_op_result = UpdateOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        FakeCheckUserAdmin::allow_all(),
    )
    .execute(
        &ctx(),
        &client.client_id(),
        UpdateOAuthClientCommand {
            name: Some(client.name().clone()),
            redirect_uris: Some(client.redirect_uris().as_set().clone()),
            tos_uri: Some(client.tos_uri().clone()),
            policy_uri: Some(client.policy_uri().clone()),
            client_uri: Some(client.client_uri().clone()),
            logo_uri: Some(client.logo_uri().clone()),
            scopes: Some(client.scopes().clone()),
        },
    )
    .await;

    assert!(no_op_result.is_ok());
    {
        let state = lock(&ports.0);
        assert_eq!(0, state.client_updates);
        assert_eq!(1, state.transaction_begins);
        assert_eq!(1, state.transaction_commits);
    }

    let update_result = UpdateOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        FakeCheckUserAdmin::allow_all(),
    )
    .execute(
        &ctx(),
        &client.client_id(),
        UpdateOAuthClientCommand {
            name: Some(OAuthClientName::from("Updated Client")),
            redirect_uris: None,
            tos_uri: None,
            policy_uri: None,
            client_uri: None,
            logo_uri: None,
            scopes: None,
        },
    )
    .await;

    assert!(update_result.is_ok());
    let state = lock(&ports.0);
    assert!(
        state
            .client
            .as_ref()
            .is_some_and(|client| client.name() == &OAuthClientName::from("Updated Client"))
    );
    assert!(
        state
            .client
            .as_ref()
            .is_some_and(|client| raw_secret.check(client.hashed_client_secret()))
    );
    assert_eq!(1, state.client_updates);
    assert_eq!(2, state.transaction_begins);
    assert_eq!(2, state.transaction_commits);
}

#[tokio::test]
async fn should_reject_invalid_client_metadata() {
    let ports = FakePorts::default();
    let err = CreateOAuthClientHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports,
        FakeCheckUserAdmin::allow_all(),
    )
    .execute(
        &ctx(),
        CreateOAuthClientCommand {
            name: OAuthClientName::from("A"),
            redirect_uris: HashSet::new(),
            tos_uri: url("https://client.example/tos"),
            policy_uri: url("https://client.example/policy"),
            client_uri: url("https://client.example"),
            logo_uri: url("https://client.example/logo.png"),
            scopes: HashSet::new(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, OAuthServiceError::InvalidClientMetadata(_)));
}

#[tokio::test]
async fn should_commit_authorization_code_exchange_when_valid() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    lock(&ports.0).client = Some(client.clone());
    let authorize =
        AuthorizeHandler::new(FakeUnitOfWork(ports.clone()), ports.clone(), ports.clone());
    let auth = authorize
        .execute(
            &ctx(),
            AuthorizeRequest {
                response_type: OAuthResponseType::Code,
                client_id: client.client_id(),
                redirect_uri: url("https://client.example/callback"),
                scope: HashSet::from([Scope::ProductListingsWrite]),
                state: Some(OAuthState::from("state-1")),
                code_challenge: OAuthCodeChallenge::from(
                    "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
                ),
                code_challenge_method: CodeChallengeMethod::S256,
            },
        )
        .await
        .unwrap();
    assert!(auth.redirect_to.contains("state=state-1"));
    {
        let state = lock(&ports.0);
        assert_eq!(1, state.transaction_begins);
        assert_eq!(1, state.transaction_commits);
    }
    let code = lock(&ports.0).code.clone().unwrap().code();
    let token = TokenByAuthorizationCodeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
    )
    .execute(TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code,
        redirect_uri: url("https://client.example/callback"),
        client_id: client.client_id(),
        client_secret: raw_secret,
        code_verifier: OAuthCodeVerifier::from("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
    })
    .await
    .unwrap();
    assert!(matches!(token.token_type, OAuthTokenType::Bearer));
    assert!(token.third_party_exchange_code.is_some());
    let state = lock(&ports.0);
    assert!(state.code.is_none());
    assert!(state.issued.is_some());
    assert!(state.exchange.is_some());
    assert_eq!(2, state.transaction_begins);
    assert_eq!(2, state.transaction_commits);
}

#[tokio::test]
async fn should_commit_consumed_authorization_code_when_expired() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    let code = authorization_code(
        client.client_id(),
        OffsetDateTime::now_utc() - time::Duration::minutes(1),
    );
    {
        let mut state = lock(&ports.0);
        state.client = Some(client.clone());
        state.code = Some(code.clone());
    }

    let error = TokenByAuthorizationCodeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
    )
    .execute(TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code: code.code(),
        redirect_uri: url("https://client.example/callback"),
        client_id: client.client_id(),
        client_secret: raw_secret,
        code_verifier: OAuthCodeVerifier::from("invalid-for-expired-code"),
    })
    .await
    .unwrap_err();

    assert!(matches!(error, OAuthServiceError::AuthorizationCodeExpired));
    let state = lock(&ports.0);
    assert!(state.code.is_none());
    assert!(state.issued.is_none());
    assert!(state.exchange.is_none());
    assert_eq!(1, state.transaction_begins);
    assert_eq!(1, state.transaction_commits);
}

#[tokio::test]
async fn should_commit_consumed_authorization_code_when_client_mismatches() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    let code = authorization_code(
        OAuthClientId::new(),
        OffsetDateTime::now_utc() + time::Duration::minutes(1),
    );
    lock(&ports.0).client = Some(client.clone());
    lock(&ports.0).code = Some(code.clone());

    let (error, state) = redeem_authorization_code(
        ports,
        code,
        client.client_id(),
        raw_secret,
        url("https://client.example/callback"),
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    )
    .await;

    assert!(matches!(
        error,
        OAuthServiceError::AuthorizationCodeClientMismatch
    ));
    assert!(state.code.is_none());
    assert!(state.issued.is_none());
    assert!(state.exchange.is_none());
    assert_eq!(1, state.transaction_commits);
}

#[tokio::test]
async fn should_commit_consumed_authorization_code_when_redirect_uri_mismatches() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    let code = authorization_code(
        client.client_id(),
        OffsetDateTime::now_utc() + time::Duration::minutes(1),
    );
    lock(&ports.0).client = Some(client.clone());
    lock(&ports.0).code = Some(code.clone());

    let (error, state) = redeem_authorization_code(
        ports,
        code,
        client.client_id(),
        raw_secret,
        url("https://client.example/other-callback"),
        "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
    )
    .await;

    assert!(matches!(
        error,
        OAuthServiceError::AuthorizationCodeRedirectUriMismatch
    ));
    assert!(state.code.is_none());
    assert!(state.issued.is_none());
    assert!(state.exchange.is_none());
    assert_eq!(1, state.transaction_commits);
}

#[tokio::test]
async fn should_commit_consumed_authorization_code_when_pkce_verifier_is_invalid() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    let code = authorization_code(
        client.client_id(),
        OffsetDateTime::now_utc() + time::Duration::minutes(1),
    );
    lock(&ports.0).client = Some(client.clone());
    lock(&ports.0).code = Some(code.clone());

    let (error, state) = redeem_authorization_code(
        ports,
        code,
        client.client_id(),
        raw_secret,
        url("https://client.example/callback"),
        "invalid-pkce-verifier",
    )
    .await;

    assert!(matches!(error, OAuthServiceError::InvalidCodeVerifier));
    assert!(state.code.is_none());
    assert!(state.issued.is_none());
    assert!(state.exchange.is_none());
    assert_eq!(1, state.transaction_commits);
}

#[tokio::test]
async fn should_roll_back_authorization_code_consumption_when_access_token_insert_fails() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    let code = authorization_code(
        client.client_id(),
        OffsetDateTime::now_utc() + time::Duration::minutes(1),
    );
    {
        let mut state = lock(&ports.0);
        state.client = Some(client.clone());
        state.code = Some(code.clone());
        state.fail_access_token_insert = true;
    }

    let error = TokenByAuthorizationCodeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
    )
    .execute(TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code: code.code(),
        redirect_uri: url("https://client.example/callback"),
        client_id: client.client_id(),
        client_secret: raw_secret,
        code_verifier: OAuthCodeVerifier::from("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
    })
    .await
    .unwrap_err();

    assert!(matches!(error, OAuthServiceError::Internal { .. }));
    let state = lock(&ports.0);
    assert_eq!(Some(code), state.code);
    assert!(state.issued.is_none());
    assert!(state.exchange.is_none());
    assert_eq!(0, state.transaction_commits);
}

#[tokio::test]
async fn should_roll_back_authorization_code_consumption_when_exchange_code_insert_fails() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    let code = authorization_code(
        client.client_id(),
        OffsetDateTime::now_utc() + time::Duration::minutes(1),
    );
    {
        let mut state = lock(&ports.0);
        state.client = Some(client.clone());
        state.code = Some(code.clone());
        state.fail_exchange_insert = true;
    }

    let error = TokenByAuthorizationCodeHandler::new(
        FakeUnitOfWork(ports.clone()),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
    )
    .execute(TokenByAuthorizationCodeRequest {
        grant_type: OAuthGrantType::AuthorizationCode,
        code: code.code(),
        redirect_uri: url("https://client.example/callback"),
        client_id: client.client_id(),
        client_secret: raw_secret,
        code_verifier: OAuthCodeVerifier::from("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
    })
    .await
    .unwrap_err();

    assert!(matches!(error, OAuthServiceError::Internal { .. }));
    let state = lock(&ports.0);
    assert_eq!(Some(code), state.code);
    assert!(state.issued.is_none());
    assert!(state.exchange.is_none());
    assert_eq!(1, state.transaction_begins);
    assert_eq!(0, state.transaction_commits);
}

#[tokio::test]
async fn should_reject_authorize_with_invalid_scope() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    lock(&ports.0).client = Some(client.clone());
    let err = AuthorizeHandler::new(FakeUnitOfWork(ports.clone()), ports.clone(), ports.clone())
        .execute(
            &context(Principal::User(UserId::new())),
            AuthorizeRequest {
                response_type: OAuthResponseType::Code,
                client_id: client.client_id(),
                redirect_uri: url("https://client.example/callback"),
                scope: HashSet::from([Scope::UsersRead]),
                state: None,
                code_challenge: OAuthCodeChallenge::from("challenge"),
                code_challenge_method: CodeChallengeMethod::S256,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, OAuthServiceError::InvalidScope));
    let state = lock(&ports.0);
    assert_eq!(1, state.transaction_begins);
    assert_eq!(0, state.transaction_commits);
}

#[tokio::test]
async fn should_allow_authorize_for_a_direct_user_principal() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    lock(&ports.0).client = Some(client.clone());

    let result = AuthorizeHandler::new(FakeUnitOfWork(ports.clone()), ports.clone(), ports.clone())
        .execute(
            &context(Principal::User(UserId::new())),
            authorize_request(
                client.client_id(),
                HashSet::from([Scope::ProductListingsWrite]),
            ),
        )
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn should_reject_authorize_for_anonymous_principal_before_transaction() {
    let ports = FakePorts::default();
    let result = AuthorizeHandler::new(FakeUnitOfWork(ports.clone()), ports.clone(), ports.clone())
        .execute(
            &context(Principal::Anonymous),
            authorize_request(OAuthClientId::new(), HashSet::new()),
        )
        .await;

    assert!(matches!(
        result,
        Err(OAuthServiceError::AuthenticatedActorRequired)
    ));
    assert_eq!(0, lock(&ports.0).transaction_begins);
}

#[tokio::test]
async fn should_reject_authorize_for_delegated_principal_without_write_capability() {
    let ports = FakePorts::default();
    let result = AuthorizeHandler::new(FakeUnitOfWork(ports.clone()), ports.clone(), ports.clone())
        .execute(
            &context(Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: BTreeSet::from([Scope::ProductListingsWrite]),
            }),
            authorize_request(OAuthClientId::new(), HashSet::new()),
        )
        .await;

    assert!(matches!(result, Err(OAuthServiceError::Forbidden)));
    assert_eq!(0, lock(&ports.0).transaction_begins);
}

#[tokio::test]
async fn should_reject_delegated_authorize_scope_outside_caller_capabilities() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    lock(&ports.0).client = Some(client.clone());

    let result = AuthorizeHandler::new(FakeUnitOfWork(ports.clone()), ports.clone(), ports.clone())
        .execute(
            &context(Principal::DelegatedUser {
                user_id: UserId::new(),
                capabilities: BTreeSet::from([CredentialCapability::AccessTokensWrite]),
            }),
            authorize_request(
                client.client_id(),
                HashSet::from([Scope::ProductListingsWrite]),
            ),
        )
        .await;

    assert!(matches!(result, Err(OAuthServiceError::Forbidden)));
    assert_eq!(0, lock(&ports.0).transaction_begins);
}

#[tokio::test]
async fn should_exchange_third_party_code_once() {
    let ports = FakePorts::default();
    let grant = ThirdPartyExchangeCodeGrant::create(RehydratedThirdPartyExchangeCodeGrantState {
        code: ThirdPartyExchangeCode::new(),
        access_token_id: AccessTokenId::new(),
        access_token: RawAccessToken::new(),
        access_token_expires: None,
        scopes: HashSet::from([Scope::ProductListingsWrite]),
        expires: OffsetDateTime::now_utc() + time::Duration::minutes(1),
    });
    lock(&ports.0).exchange = Some(grant.clone());
    let response = TokenByThirdPartyCodeHandler::new(FakeUnitOfWork(ports.clone()), ports.clone())
        .execute(&grant.code())
        .await
        .unwrap();
    assert_eq!(grant.access_token().clone(), response.access_token);
    assert!(lock(&ports.0).exchange.is_none());
    assert_eq!(1, lock(&ports.0).transaction_begins);
    assert_eq!(1, lock(&ports.0).transaction_commits);
}

#[tokio::test]
async fn should_commit_consumed_third_party_exchange_code_when_expired() {
    let ports = FakePorts::default();
    let grant = ThirdPartyExchangeCodeGrant::create(RehydratedThirdPartyExchangeCodeGrantState {
        code: ThirdPartyExchangeCode::new(),
        access_token_id: AccessTokenId::new(),
        access_token: RawAccessToken::new(),
        access_token_expires: None,
        scopes: HashSet::from([Scope::ProductListingsWrite]),
        expires: OffsetDateTime::now_utc() - time::Duration::minutes(1),
    });
    lock(&ports.0).exchange = Some(grant.clone());

    let error = TokenByThirdPartyCodeHandler::new(FakeUnitOfWork(ports.clone()), ports.clone())
        .execute(&grant.code())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        OAuthServiceError::ThirdPartyExchangeCodeExpired
    ));
    let state = lock(&ports.0);
    assert!(state.exchange.is_none());
    assert!(state.issued.is_none());
    assert_eq!(1, state.transaction_begins);
    assert_eq!(1, state.transaction_commits);
}

#[tokio::test]
async fn should_revoke_and_introspect_tokens() {
    let ports = FakePorts::default();
    let raw_secret = RawOAuthClientSecret::new();
    let client = client_with_secret(&raw_secret);
    let raw_access_token = RawAccessToken::new();
    let issued = AccessToken::create(NewAccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw_access_token.clone().into(),
        user_id: UserId::new(),
        name: AccessTokenName::from("OAuth test token"),
        scopes: HashSet::from([Scope::ProductListingsWrite]),
        origin: AccessTokenOrigin::OAuth {
            client_id: client.client_id(),
        },
        expires: None,
    });
    {
        let mut s = lock(&ports.0);
        s.client = Some(client.clone());
        s.issued = Some(issued);
    }
    let active = IntrospectTokenHandler::new(ports.clone(), ports.clone())
        .execute(IntrospectTokenRequest {
            token: raw_access_token.clone(),
            client_id: client.client_id(),
            client_secret: raw_secret.clone(),
        })
        .await
        .unwrap();
    assert!(active.active);
    assert_eq!(Some(client.client_id()), active.client_id);
    assert_eq!(0, lock(&ports.0).transaction_begins);
    RevokeTokenHandler::new(FakeUnitOfWork(ports.clone()), ports.clone(), ports.clone())
        .execute(
            &ctx(),
            RevokeTokenRequest {
                token: raw_access_token.clone(),
                client_id: client.client_id(),
                client_secret: raw_secret.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(1, lock(&ports.0).deleted_raw);
    let inactive = IntrospectTokenHandler::new(ports.clone(), ports.clone())
        .execute(IntrospectTokenRequest {
            token: RawAccessToken::new(),
            client_id: client.client_id(),
            client_secret: raw_secret,
        })
        .await
        .unwrap();
    assert!(!inactive.active);
}

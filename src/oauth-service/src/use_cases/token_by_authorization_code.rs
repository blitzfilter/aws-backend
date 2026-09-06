use crate::error::OAuthServiceError;
use crate::ports::{
    AuthorizationCodeRepository, AuthorizationCodeRepositoryFactory, OAuthClientRepositoryFactory,
    ThirdPartyExchangeCodeRepository, ThirdPartyExchangeCodeRepositoryFactory,
};
use crate::use_cases::support::{THIRD_PARTY_EXCHANGE_CODE_TTL, authenticate_client, verify_s256};
use application::transaction::{Transaction, UnitOfWork};
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{OAuthAuthorizationCode, OAuthCodeVerifier};
use oauth_core::third_party_exchange_code::{
    RehydratedThirdPartyExchangeCodeGrantState, ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant,
};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, NewAccessToken, RawAccessToken,
    RawOAuthClientSecret, Scope,
};
use user_service::ports::{AccessTokenRepository, AccessTokenRepositoryFactory};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthGrantType {
    AuthorizationCode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthTokenType {
    Bearer,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenByAuthorizationCodeRequest {
    pub grant_type: OAuthGrantType,
    pub code: OAuthAuthorizationCode,
    pub redirect_uri: url::Url,
    pub client_id: OAuthClientId,
    pub client_secret: RawOAuthClientSecret,
    pub code_verifier: OAuthCodeVerifier,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenResponse {
    pub access_token: RawAccessToken,
    pub token_type: OAuthTokenType,
    pub expires: Option<OffsetDateTime>,
    pub scopes: HashSet<Scope>,
    pub third_party_exchange_code: Option<ThirdPartyExchangeCode>,
}

#[async_trait::async_trait]
pub trait TokenByAuthorizationCodeUseCase: Send + Sync {
    async fn execute(
        &self,
        request: TokenByAuthorizationCodeRequest,
    ) -> Result<TokenResponse, OAuthServiceError>;
}

pub struct TokenByAuthorizationCodeHandler<U, C, A, T, R> {
    unit_of_work: U,
    clients: C,
    codes: A,
    exchange_codes: T,
    access_tokens: R,
}
impl<U, C, A, T, R> TokenByAuthorizationCodeHandler<U, C, A, T, R> {
    pub fn new(unit_of_work: U, clients: C, codes: A, exchange_codes: T, access_tokens: R) -> Self {
        Self {
            unit_of_work,
            clients,
            codes,
            exchange_codes,
            access_tokens,
        }
    }
}
#[async_trait::async_trait]
impl<U, C, A, T, R> TokenByAuthorizationCodeUseCase
    for TokenByAuthorizationCodeHandler<U, C, A, T, R>
where
    U: UnitOfWork,
    C: OAuthClientRepositoryFactory<U::Tx>,
    A: AuthorizationCodeRepositoryFactory<U::Tx>,
    T: ThirdPartyExchangeCodeRepositoryFactory<U::Tx>,
    R: AccessTokenRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        request: TokenByAuthorizationCodeRequest,
    ) -> Result<TokenResponse, OAuthServiceError> {
        let mut tx = self.unit_of_work.begin().await?;
        let client = authenticate_client(
            &mut self.clients.in_transaction(&mut tx),
            &request.client_id,
            &request.client_secret,
        )
        .await?;
        let code = self
            .codes
            .in_transaction(&mut tx)
            .consume_by_code(&request.code)
            .await?
            .ok_or(OAuthServiceError::AuthorizationCodeNotFound)?;
        if code.is_expired_at(OffsetDateTime::now_utc()) {
            tx.commit().await?;
            return Err(OAuthServiceError::AuthorizationCodeExpired);
        }
        if code.client_id() != request.client_id {
            tx.commit().await?;
            return Err(OAuthServiceError::AuthorizationCodeClientMismatch);
        }
        if code.redirect_uri() != &request.redirect_uri {
            tx.commit().await?;
            return Err(OAuthServiceError::AuthorizationCodeRedirectUriMismatch);
        }
        if !verify_s256(&request.code_verifier, code.code_challenge()) {
            tx.commit().await?;
            return Err(OAuthServiceError::InvalidCodeVerifier);
        }
        let raw_access_token = RawAccessToken::new();
        let issued_at = OffsetDateTime::now_utc();
        let access_token = AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw_access_token.clone().into(),
            user_id: code.user_id(),
            name: AccessTokenName::from(
                format!("{} (OAuth-Client {})", client.name(), client.client_id()).as_str(),
            ),
            scopes: code.scopes().clone(),
            origin: AccessTokenOrigin::OAuth {
                client_id: client.client_id(),
            },
            expires: None,
        });
        self.access_tokens
            .in_transaction(&mut tx)
            .insert(&access_token)
            .await?;
        let third_party_exchange_code =
            ThirdPartyExchangeCodeGrant::create(RehydratedThirdPartyExchangeCodeGrantState {
                code: ThirdPartyExchangeCode::new(),
                access_token_id: access_token.id(),
                access_token: raw_access_token.clone(),
                access_token_expires: access_token.expires(),
                scopes: access_token.scopes().clone(),
                expires: issued_at + THIRD_PARTY_EXCHANGE_CODE_TTL,
            });
        self.exchange_codes
            .in_transaction(&mut tx)
            .insert(third_party_exchange_code.clone())
            .await?;
        tx.commit().await?;
        Ok(TokenResponse {
            access_token: raw_access_token,
            token_type: OAuthTokenType::Bearer,
            expires: access_token.expires(),
            scopes: access_token.scopes().clone(),
            third_party_exchange_code: Some(third_party_exchange_code.code()),
        })
    }
}

use credential_core::scope::Scope;
use domain_primitives::uuid_v7_newtype;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{AccessTokenId, RawAccessToken};

uuid_v7_newtype!(ThirdPartyExchangeCode);

#[derive(Debug, Clone, PartialEq)]
pub struct ThirdPartyExchangeCodeGrant {
    code: ThirdPartyExchangeCode,
    access_token_id: AccessTokenId,
    access_token: RawAccessToken,
    access_token_expires: Option<OffsetDateTime>,
    scopes: HashSet<Scope>,
    expires: OffsetDateTime,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedThirdPartyExchangeCodeGrantState {
    pub code: ThirdPartyExchangeCode,
    pub access_token_id: AccessTokenId,
    pub access_token: RawAccessToken,
    pub access_token_expires: Option<OffsetDateTime>,
    pub scopes: HashSet<Scope>,
    pub expires: OffsetDateTime,
}

impl ThirdPartyExchangeCodeGrant {
    pub fn create(state: RehydratedThirdPartyExchangeCodeGrantState) -> Self {
        Self::rehydrate(state)
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedThirdPartyExchangeCodeGrantState) -> Self {
        Self {
            code: state.code,
            access_token_id: state.access_token_id,
            access_token: state.access_token,
            access_token_expires: state.access_token_expires,
            scopes: state.scopes,
            expires: state.expires,
        }
    }

    pub fn code(&self) -> ThirdPartyExchangeCode {
        self.code
    }

    pub fn access_token_id(&self) -> AccessTokenId {
        self.access_token_id
    }

    pub fn access_token(&self) -> &RawAccessToken {
        &self.access_token
    }

    pub fn access_token_expires(&self) -> Option<OffsetDateTime> {
        self.access_token_expires
    }

    pub fn scopes(&self) -> &HashSet<Scope> {
        &self.scopes
    }

    pub fn expires(&self) -> OffsetDateTime {
        self.expires
    }

    pub fn is_expired_at(&self, now: OffsetDateTime) -> bool {
        self.expires < now
    }
}

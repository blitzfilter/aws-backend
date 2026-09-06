use crate::error::OAuthServiceError;
use crate::ports::{OAuthClientAuthenticationReader, OAuthClientRepository};
use application::error::static_error;
use application::operation_context::{CredentialCapability, OperationContext, Principal};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use credential_core::oauth_client_id::OAuthClientId;
use oauth_core::authorization_code::{OAuthCodeChallenge, OAuthCodeVerifier};
use oauth_core::client::OAuthClient;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use user_core::access_token::RawOAuthClientSecret;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

pub(crate) const AUTHORIZATION_CODE_TTL: time::Duration = time::Duration::minutes(10);
pub(crate) const THIRD_PARTY_EXCHANGE_CODE_TTL: time::Duration = time::Duration::seconds(60);

pub(crate) fn authorize_oauth_admin(context: &OperationContext) -> Result<(), OAuthServiceError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensWrite)
        .authorize::<OAuthServiceError>()
}

pub(crate) fn authorize_oauth_client_read(
    context: &OperationContext,
) -> Result<(), OAuthServiceError> {
    context
        .require()
        .credential_capability(CredentialCapability::AccessTokensRead)
        .authorize::<OAuthServiceError>()
}

pub(crate) async fn authorize_oauth_client_admin<A>(
    context: &OperationContext,
    check_user_admin: &A,
) -> Result<(), OAuthServiceError>
where
    A: CheckUserAdminUseCase,
{
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        Principal::User(_) | Principal::DelegatedUser { .. } => check_user_admin
            .execute(context, CheckUserAdminRequest)
            .await
            .map(|_| ())
            .map_err(OAuthServiceError::from),
        Principal::Anonymous => Err(OAuthServiceError::AuthenticatedActorRequired),
    }
}

impl From<CheckUserAdminError> for OAuthServiceError {
    fn from(error: CheckUserAdminError) -> Self {
        match error {
            CheckUserAdminError::AuthenticatedActorRequired => Self::AuthenticatedActorRequired,
            CheckUserAdminError::Forbidden => Self::Forbidden,
            CheckUserAdminError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            CheckUserAdminError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
            CheckUserAdminError::Internal { source } => Self::Internal { source },
            CheckUserAdminError::BeginTransactionFailed => Self::TemporarilyUnavailable {
                source: static_error("check user admin transaction begin failed"),
            },
            CheckUserAdminError::CommitTransactionFailed => Self::TemporarilyUnavailable {
                source: static_error("check user admin transaction commit failed"),
            },
        }
    }
}

pub(crate) async fn authenticate_client<R: OAuthClientRepository>(
    repository: &mut R,
    client_id: &OAuthClientId,
    client_secret: &RawOAuthClientSecret,
) -> Result<OAuthClient, OAuthServiceError> {
    let client = repository
        .find_by_id(*client_id)
        .await?
        .ok_or(OAuthServiceError::ClientNotFound)?
        .value;
    if client_secret.check(client.hashed_client_secret()) {
        Ok(client)
    } else {
        Err(OAuthServiceError::InvalidClientSecret)
    }
}

pub(crate) async fn authenticate_client_reader<R: OAuthClientAuthenticationReader>(
    reader: &R,
    client_id: &OAuthClientId,
    client_secret: &RawOAuthClientSecret,
) -> Result<(), OAuthServiceError> {
    let client = reader
        .find_by_id(client_id)
        .await?
        .ok_or(OAuthServiceError::ClientNotFound)?;
    if client_secret.check(&client.hashed_client_secret) {
        Ok(())
    } else {
        Err(OAuthServiceError::InvalidClientSecret)
    }
}

pub(crate) fn append_query_params(uri: &url::Url, params: HashMap<&str, String>) -> String {
    let mut url = uri.clone();
    for (key, value) in params {
        url.query_pairs_mut().append_pair(key, &value);
    }
    url.to_string()
}

pub(crate) fn verify_s256(
    verifier: &OAuthCodeVerifier,
    expected_challenge: &OAuthCodeChallenge,
) -> bool {
    let digest = Sha256::digest(verifier.as_ref().as_bytes());
    URL_SAFE_NO_PAD.encode(digest) == expected_challenge.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::error::box_error;
    use application::operation_context::{CorrelationId, RequestId};
    use std::error::Error;
    use std::sync::{Arc, Mutex};
    use user_core::user_id::UserId;
    use user_service::use_cases::queries::check_user_admin::CheckUserAdminResult;

    #[derive(Clone, Copy, Debug)]
    enum CheckerOutcome {
        Success,
        AuthenticatedActorRequired,
        Forbidden,
        TemporarilyUnavailable,
        InvalidReadModel,
        Internal,
        BeginTransactionFailed,
        CommitTransactionFailed,
    }

    impl CheckerOutcome {
        fn into_result(self) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            match self {
                Self::Success => Ok(CheckUserAdminResult),
                Self::AuthenticatedActorRequired => {
                    Err(CheckUserAdminError::AuthenticatedActorRequired)
                }
                Self::Forbidden => Err(CheckUserAdminError::Forbidden),
                Self::TemporarilyUnavailable => Err(CheckUserAdminError::TemporarilyUnavailable {
                    source: box_error(std::io::Error::other("temporary")),
                }),
                Self::InvalidReadModel => Err(CheckUserAdminError::InvalidReadModel {
                    source: box_error(std::io::Error::other("invalid read model")),
                }),
                Self::Internal => Err(CheckUserAdminError::Internal {
                    source: box_error(std::io::Error::other("internal")),
                }),
                Self::BeginTransactionFailed => Err(CheckUserAdminError::BeginTransactionFailed),
                Self::CommitTransactionFailed => Err(CheckUserAdminError::CommitTransactionFailed),
            }
        }
    }

    #[derive(Clone)]
    struct RecordingChecker {
        calls: Arc<Mutex<Vec<OperationContext>>>,
        outcome: CheckerOutcome,
    }

    impl RecordingChecker {
        fn new(outcome: CheckerOutcome) -> Self {
            Self {
                calls: Arc::new(Mutex::new(Vec::new())),
                outcome,
            }
        }

        fn calls(&self) -> Vec<OperationContext> {
            match self.calls.lock() {
                Ok(calls) => calls.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }
    }

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for RecordingChecker {
        async fn execute(
            &self,
            context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            match self.calls.lock() {
                Ok(mut calls) => calls.push(context.clone()),
                Err(poisoned) => poisoned.into_inner().push(context.clone()),
            }
            self.outcome.into_result()
        }
    }

    #[derive(Clone, Copy)]
    struct NeverCalledChecker;

    #[async_trait::async_trait]
    impl CheckUserAdminUseCase for NeverCalledChecker {
        async fn execute(
            &self,
            _context: &OperationContext,
            _request: CheckUserAdminRequest,
        ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
            panic!("OAuth admin authorization must not check service, system, or anonymous actors")
        }
    }

    fn context(principal: Principal) -> OperationContext {
        OperationContext {
            principal,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    #[tokio::test]
    async fn should_skip_admin_lookup_for_service_and_system_principals() {
        for principal in [
            Principal::Service("oauth-test".to_owned()),
            Principal::System,
        ] {
            let checker = NeverCalledChecker;
            assert!(
                authorize_oauth_client_admin(&context(principal), &checker)
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    async fn should_reject_anonymous_oauth_client_admin_without_admin_lookup() {
        let checker = NeverCalledChecker;

        assert!(matches!(
            authorize_oauth_client_admin(&context(Principal::Anonymous), &checker).await,
            Err(OAuthServiceError::AuthenticatedActorRequired)
        ));
    }

    #[tokio::test]
    async fn should_delegate_user_and_delegated_user_admin_check_to_check_user_admin() {
        let user_id = UserId::new();
        for principal in [
            Principal::User(user_id),
            Principal::DelegatedUser {
                user_id,
                capabilities: Default::default(),
            },
        ] {
            let context = context(principal);
            let checker = RecordingChecker::new(CheckerOutcome::Success);

            assert!(
                authorize_oauth_client_admin(&context, &checker)
                    .await
                    .is_ok()
            );
            assert_eq!(vec![context], checker.calls());
        }
    }

    #[rstest::rstest]
    #[case(CheckerOutcome::AuthenticatedActorRequired)]
    #[case(CheckerOutcome::Forbidden)]
    #[case(CheckerOutcome::TemporarilyUnavailable)]
    #[case(CheckerOutcome::InvalidReadModel)]
    #[case(CheckerOutcome::Internal)]
    #[case(CheckerOutcome::BeginTransactionFailed)]
    #[case(CheckerOutcome::CommitTransactionFailed)]
    #[tokio::test]
    async fn should_map_check_user_admin_errors_to_oauth_service_errors(
        #[case] outcome: CheckerOutcome,
    ) {
        let checker = RecordingChecker::new(outcome);
        let error =
            authorize_oauth_client_admin(&context(Principal::User(UserId::new())), &checker)
                .await
                .expect_err("admin checker outcome must be translated");

        match outcome {
            CheckerOutcome::AuthenticatedActorRequired => {
                assert!(matches!(
                    &error,
                    OAuthServiceError::AuthenticatedActorRequired
                ));
            }
            CheckerOutcome::Forbidden => {
                assert!(matches!(&error, OAuthServiceError::Forbidden));
            }
            CheckerOutcome::TemporarilyUnavailable
            | CheckerOutcome::BeginTransactionFailed
            | CheckerOutcome::CommitTransactionFailed => {
                assert!(matches!(
                    &error,
                    OAuthServiceError::TemporarilyUnavailable { .. }
                ));
                assert!(error.source().is_some());
            }
            CheckerOutcome::InvalidReadModel => {
                assert!(matches!(
                    &error,
                    OAuthServiceError::InvalidPersistedState { .. }
                ));
                assert!(error.source().is_some());
            }
            CheckerOutcome::Internal => {
                assert!(matches!(&error, OAuthServiceError::Internal { .. }));
                assert!(error.source().is_some());
            }
            CheckerOutcome::Success => unreachable!("success is not an error-table case"),
        }
        assert_eq!(1, checker.calls().len());
    }
}

use application::error::{box_error, static_error};
use aws_sdk_cognitoidentityprovider::Client;
use user_service::ports::{
    CognitoIdentity, CognitoSubject, UserSessionRevocationError, UserSessionRevoker,
};

pub struct CognitoUserSessionRevoker {
    provider: Box<dyn CognitoProvider>,
    user_pool_id: String,
    issuer: Option<String>,
}

impl CognitoUserSessionRevoker {
    pub fn new(client: Client, user_pool_id: impl Into<String>) -> Self {
        let user_pool_id = user_pool_id.into();
        let issuer = client.config().region().map(|region| {
            format!(
                "https://cognito-idp.{}.amazonaws.com/{user_pool_id}",
                region.as_ref()
            )
        });
        Self {
            provider: Box::new(AwsCognitoProvider { client }),
            user_pool_id,
            issuer,
        }
    }

    #[cfg(test)]
    fn with_provider(
        provider: impl CognitoProvider + 'static,
        user_pool_id: impl Into<String>,
        issuer: impl Into<String>,
    ) -> Self {
        Self {
            provider: Box::new(provider),
            user_pool_id: user_pool_id.into(),
            issuer: Some(issuer.into()),
        }
    }
}

#[async_trait::async_trait]
trait CognitoProvider: Send + Sync {
    async fn username_for(
        &self,
        user_pool_id: &str,
        subject: &CognitoSubject,
    ) -> Result<String, UserSessionRevocationError>;

    async fn revoke_sessions(
        &self,
        user_pool_id: &str,
        username: &str,
    ) -> Result<(), UserSessionRevocationError>;
}

struct AwsCognitoProvider {
    client: Client,
}

#[async_trait::async_trait]
impl CognitoProvider for AwsCognitoProvider {
    async fn username_for(
        &self,
        user_pool_id: &str,
        subject: &CognitoSubject,
    ) -> Result<String, UserSessionRevocationError> {
        let response = self
            .client
            .list_users()
            .user_pool_id(user_pool_id)
            .filter(subject_filter(subject))
            .limit(2)
            .send()
            .await
            .map_err(|source| {
                let temporary = source.as_service_error().is_none_or(|error| {
                    error.is_internal_error_exception() || error.is_too_many_requests_exception()
                });
                if temporary {
                    UserSessionRevocationError::TemporarilyUnavailable {
                        source: box_error(source),
                    }
                } else {
                    UserSessionRevocationError::Internal {
                        source: box_error(source),
                    }
                }
            })?;

        unique_username(response.users())
    }

    async fn revoke_sessions(
        &self,
        user_pool_id: &str,
        username: &str,
    ) -> Result<(), UserSessionRevocationError> {
        self.client
            .admin_user_global_sign_out()
            .user_pool_id(user_pool_id)
            .username(username)
            .send()
            .await
            .map_err(|source| {
                if source
                    .as_service_error()
                    .is_some_and(|error| error.is_user_not_found_exception())
                {
                    return UserSessionRevocationError::UserNotFound;
                }
                let temporary = source.as_service_error().is_none_or(|error| {
                    error.is_internal_error_exception() || error.is_too_many_requests_exception()
                });
                if temporary {
                    UserSessionRevocationError::TemporarilyUnavailable {
                        source: box_error(source),
                    }
                } else {
                    UserSessionRevocationError::Internal {
                        source: box_error(source),
                    }
                }
            })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl UserSessionRevoker for CognitoUserSessionRevoker {
    async fn revoke_sessions(
        &self,
        identity: &CognitoIdentity,
    ) -> Result<(), UserSessionRevocationError> {
        if self.issuer.as_deref() != Some(identity.issuer.as_str()) {
            return Err(UserSessionRevocationError::Internal {
                source: static_error(
                    "persisted Cognito issuer does not match configured user pool",
                ),
            });
        }

        let username = self
            .provider
            .username_for(&self.user_pool_id, &identity.subject)
            .await?;
        self.provider
            .revoke_sessions(&self.user_pool_id, &username)
            .await
    }
}

fn unique_username(
    users: &[aws_sdk_cognitoidentityprovider::types::UserType],
) -> Result<String, UserSessionRevocationError> {
    let [user] = users else {
        return if users.is_empty() {
            Err(UserSessionRevocationError::UserNotFound)
        } else {
            Err(UserSessionRevocationError::Internal {
                source: static_error("Cognito returned multiple users for one subject"),
            })
        };
    };

    user.username()
        .map(ToOwned::to_owned)
        .ok_or_else(|| UserSessionRevocationError::Internal {
            source: static_error("Cognito user has no username"),
        })
}

fn subject_filter(subject: &CognitoSubject) -> String {
    let mut escaped = String::with_capacity(subject.as_str().len());
    for character in subject.as_str().chars() {
        if matches!(character, '\\' | '"') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    format!("sub = \"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::task::{Context, Poll, Waker};
    use user_service::ports::CognitoIssuer;

    const USER_POOL_ID: &str = "eu-central-1_test-pool";
    const ISSUER: &str = "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_test-pool";

    fn subject(value: &str) -> CognitoSubject {
        CognitoSubject::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test subject: {error}"))
    }

    fn identity(issuer: &str, subject: &str) -> CognitoIdentity {
        CognitoIdentity {
            issuer: CognitoIssuer::try_from(issuer)
                .unwrap_or_else(|error| panic!("invalid test issuer: {error}")),
            subject: self::subject(subject),
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeProvider {
        lookups: Arc<AtomicUsize>,
        revocations: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl CognitoProvider for FakeProvider {
        async fn username_for(
            &self,
            _: &str,
            _: &CognitoSubject,
        ) -> Result<String, UserSessionRevocationError> {
            self.lookups.fetch_add(1, Ordering::Relaxed);
            Ok("provider-username".to_owned())
        }

        async fn revoke_sessions(
            &self,
            _: &str,
            _: &str,
        ) -> Result<(), UserSessionRevocationError> {
            self.revocations.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn should_reject_same_subject_from_wrong_issuer_before_provider_call() {
        let provider = FakeProvider::default();
        let revoker =
            CognitoUserSessionRevoker::with_provider(provider.clone(), USER_POOL_ID, ISSUER);
        let matching_identity = identity(ISSUER, "provider|shared-subject");
        let wrong_issuer_identity = identity(
            "https://cognito-idp.eu-central-1.amazonaws.com/eu-central-1_other-pool",
            "provider|shared-subject",
        );
        assert_eq!(matching_identity.subject, wrong_issuer_identity.subject);

        let result = block_on(UserSessionRevoker::revoke_sessions(
            &revoker,
            &wrong_issuer_identity,
        ));

        assert!(matches!(
            result,
            Err(UserSessionRevocationError::Internal { .. })
        ));
        assert_eq!(0, provider.lookups.load(Ordering::Relaxed));
        assert_eq!(0, provider.revocations.load(Ordering::Relaxed));
    }

    #[test]
    fn should_lookup_and_revoke_when_issuer_matches() {
        let provider = FakeProvider::default();
        let revoker =
            CognitoUserSessionRevoker::with_provider(provider.clone(), USER_POOL_ID, ISSUER);

        let result = block_on(UserSessionRevoker::revoke_sessions(
            &revoker,
            &identity(ISSUER, "provider|opaque-subject"),
        ));

        assert!(result.is_ok());
        assert_eq!(1, provider.lookups.load(Ordering::Relaxed));
        assert_eq!(1, provider.revocations.load(Ordering::Relaxed));
    }

    #[test]
    fn should_create_sub_filter_from_opaque_subject() {
        assert_eq!(
            "sub = \"provider|not-a-uuid\"",
            subject_filter(&subject("provider|not-a-uuid"))
        );
    }

    #[test]
    fn should_escape_filter_metacharacters_in_opaque_subject() {
        assert_eq!(
            "sub = \"quoted\\\"subject\\\\suffix\"",
            subject_filter(&subject("quoted\"subject\\suffix"))
        );
    }

    #[test]
    fn should_return_only_cognito_username() {
        let users = [aws_sdk_cognitoidentityprovider::types::UserType::builder()
            .username("provider-username")
            .build()];

        assert!(matches!(
            unique_username(&users),
            Ok(username) if username == "provider-username"
        ));
    }

    #[test]
    fn should_reject_missing_duplicate_or_nameless_cognito_users() {
        let duplicate = [
            aws_sdk_cognitoidentityprovider::types::UserType::builder()
                .username("first")
                .build(),
            aws_sdk_cognitoidentityprovider::types::UserType::builder()
                .username("second")
                .build(),
        ];
        let nameless = [aws_sdk_cognitoidentityprovider::types::UserType::builder().build()];

        assert!(matches!(
            unique_username(&[]),
            Err(UserSessionRevocationError::UserNotFound)
        ));
        assert!(matches!(
            unique_username(&duplicate),
            Err(UserSessionRevocationError::Internal { .. })
        ));
        assert!(matches!(
            unique_username(&nameless),
            Err(UserSessionRevocationError::Internal { .. })
        ));
    }
}

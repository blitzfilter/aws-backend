use application::error::{box_error, static_error};
use aws_sdk_cognitoidentityprovider::Client;
use user_service::ports::{CognitoSubject, UserSessionRevocationError, UserSessionRevoker};

pub struct CognitoUserSessionRevoker {
    client: Client,
    user_pool_id: String,
}

impl CognitoUserSessionRevoker {
    pub fn new(client: Client, user_pool_id: impl Into<String>) -> Self {
        Self {
            client,
            user_pool_id: user_pool_id.into(),
        }
    }

    async fn username_for(
        &self,
        subject: &CognitoSubject,
    ) -> Result<String, UserSessionRevocationError> {
        let response = self
            .client
            .list_users()
            .user_pool_id(&self.user_pool_id)
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
}

#[async_trait::async_trait]
impl UserSessionRevoker for CognitoUserSessionRevoker {
    async fn revoke_sessions(
        &self,
        subject: &CognitoSubject,
    ) -> Result<(), UserSessionRevocationError> {
        let username = self.username_for(subject).await?;
        self.client
            .admin_user_global_sign_out()
            .user_pool_id(&self.user_pool_id)
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

    fn subject(value: &str) -> CognitoSubject {
        CognitoSubject::try_from(value)
            .unwrap_or_else(|error| panic!("invalid test subject: {error}"))
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

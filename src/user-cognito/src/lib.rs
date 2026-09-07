use application::error::{box_error, static_error};
use aws_sdk_cognitoidentityprovider::Client;
use user_core::user_id::UserId;
use user_service::ports::{UserSessionRevocationError, UserSessionRevoker};

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

    async fn username_for(&self, user_id: UserId) -> Result<String, UserSessionRevocationError> {
        let response = self
            .client
            .list_users()
            .user_pool_id(&self.user_pool_id)
            .filter(format!("sub = \"{user_id}\""))
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

        let mut users = response.users().iter();
        let user = users
            .next()
            .ok_or(UserSessionRevocationError::UserNotFound)?;
        if users.next().is_some() {
            return Err(UserSessionRevocationError::Internal {
                source: static_error("Cognito returned multiple users for one subject"),
            });
        }

        user.username()
            .map(ToOwned::to_owned)
            .ok_or_else(|| UserSessionRevocationError::Internal {
                source: static_error("Cognito user has no username"),
            })
    }
}

#[async_trait::async_trait]
impl UserSessionRevoker for CognitoUserSessionRevoker {
    async fn revoke_sessions(&self, user_id: UserId) -> Result<(), UserSessionRevocationError> {
        let username = self.username_for(user_id).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_create_sub_filter_for_user_id() {
        let user_id = UserId::try_from("550e8400-e29b-41d4-a716-446655440000")
            .unwrap_or_else(|error| panic!("invalid fixture user ID: {error}"));

        assert_eq!(
            "sub = \"550e8400-e29b-41d4-a716-446655440000\"",
            format!("sub = \"{user_id}\"")
        );
    }
}

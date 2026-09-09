use application::error::BoxError;
use user_core::user_id::UserId;

const MAX_COGNITO_IDENTITY_BYTES: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CognitoIssuer(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CognitoSubject(String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CognitoIdentity {
    pub issuer: CognitoIssuer,
    pub subject: CognitoSubject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidCognitoIdentityValue {
    #[error("Cognito identity value must not be empty")]
    Empty,
    #[error("Cognito identity value exceeds {MAX_COGNITO_IDENTITY_BYTES} bytes")]
    TooLong,
    #[error("Cognito identity value contains a control character")]
    ControlCharacter,
}

impl CognitoIssuer {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl CognitoSubject {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CognitoIssuer {
    type Error = InvalidCognitoIdentityValue;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_identity_value(&value)?;
        Ok(Self(value))
    }
}

impl TryFrom<&str> for CognitoIssuer {
    type Error = InvalidCognitoIdentityValue;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl TryFrom<String> for CognitoSubject {
    type Error = InvalidCognitoIdentityValue;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_identity_value(&value)?;
        Ok(Self(value))
    }
}

impl TryFrom<&str> for CognitoSubject {
    type Error = InvalidCognitoIdentityValue;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

fn validate_identity_value(value: &str) -> Result<(), InvalidCognitoIdentityValue> {
    if value.is_empty() {
        return Err(InvalidCognitoIdentityValue::Empty);
    }
    if value.len() > MAX_COGNITO_IDENTITY_BYTES {
        return Err(InvalidCognitoIdentityValue::TooLong);
    }
    if value.chars().any(char::is_control) {
        return Err(InvalidCognitoIdentityValue::ControlCharacter);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum UserCognitoIdentityRegistryError {
    #[error("Cognito identity is already bound")]
    Conflict {
        #[source]
        source: BoxError,
    },
    #[error("temporary Cognito identity persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted Cognito identity")]
    InvalidPersistedIdentity {
        #[source]
        source: BoxError,
    },
    #[error("internal Cognito identity persistence failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait UserCognitoIdentityRegistry: Send {
    async fn lock_and_find_user_id(
        &mut self,
        identity: &CognitoIdentity,
    ) -> Result<Option<UserId>, UserCognitoIdentityRegistryError>;

    async fn find_by_user_id(
        &mut self,
        user_id: UserId,
    ) -> Result<Option<CognitoIdentity>, UserCognitoIdentityRegistryError>;

    async fn bind(
        &mut self,
        identity: &CognitoIdentity,
        user_id: UserId,
    ) -> Result<(), UserCognitoIdentityRegistryError>;
}

pub trait UserCognitoIdentityRegistryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl UserCognitoIdentityRegistry + 'tx;
}

#[derive(Debug, thiserror::Error)]
pub enum CognitoUserIdentityReadError {
    #[error("temporary Cognito identity read failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted Cognito identity")]
    InvalidPersistedIdentity {
        #[source]
        source: BoxError,
    },
    #[error("internal Cognito identity read failure")]
    Internal {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait CognitoUserIdentityReader: Send + Sync {
    async fn find_user_id(
        &self,
        identity: &CognitoIdentity,
    ) -> Result<Option<UserId>, CognitoUserIdentityReadError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_preserve_opaque_non_uuid_subject() {
        let subject = CognitoSubject::try_from("auth0|opaque-user:42")
            .unwrap_or_else(|error| panic!("valid opaque subject rejected: {error}"));

        assert_eq!("auth0|opaque-user:42", subject.as_str());
    }

    #[test]
    fn should_accept_identity_value_at_byte_limit() {
        let value = "a".repeat(MAX_COGNITO_IDENTITY_BYTES);

        assert!(CognitoIssuer::try_from(value.as_str()).is_ok());
        assert!(CognitoSubject::try_from(value).is_ok());
    }

    #[test]
    fn should_reject_empty_oversized_or_control_identity_values() {
        assert!(matches!(
            CognitoSubject::try_from(""),
            Err(InvalidCognitoIdentityValue::Empty)
        ));
        assert!(matches!(
            CognitoIssuer::try_from("é".repeat((MAX_COGNITO_IDENTITY_BYTES / 2) + 1)),
            Err(InvalidCognitoIdentityValue::TooLong)
        ));
        assert!(matches!(
            CognitoSubject::try_from("bad\nsubject"),
            Err(InvalidCognitoIdentityValue::ControlCharacter)
        ));
    }
}

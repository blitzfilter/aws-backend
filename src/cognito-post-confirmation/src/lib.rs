use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use lambda_runtime::LambdaEvent;
use serde_email::Email;
use user_service::ports::{CognitoIdentity, CognitoIssuer, CognitoSubject};
use user_service::use_cases::{RegisterCognitoUserCommand, RegisterCognitoUserUseCase};

#[derive(Debug, thiserror::Error)]
enum PostConfirmationInputError {
    #[error("missing Cognito event field: {name}")]
    MissingField { name: &'static str },
    #[error("invalid Cognito identity")]
    InvalidIdentity,
    #[error("invalid Cognito user email")]
    InvalidEmail,
}

#[tracing::instrument(
    skip(event, service),
    fields(request_id = %event.context.request_id)
)]
pub async fn handler(
    event: LambdaEvent<CognitoEventUserPoolsPostConfirmation>,
    service: &impl RegisterCognitoUserUseCase,
) -> Result<CognitoEventUserPoolsPostConfirmation, lambda_runtime::Error> {
    let (identity, email) = parse_user(&event.payload)?;
    let request_id = event.context.request_id.clone();

    service
        .execute(
            &OperationContext {
                principal: Principal::System,
                request_id: RequestId::new(request_id.clone()),
                correlation_id: CorrelationId::new(request_id),
            },
            RegisterCognitoUserCommand { identity, email },
        )
        .await?;

    Ok(event.payload)
}

fn parse_user(
    event: &CognitoEventUserPoolsPostConfirmation,
) -> Result<(CognitoIdentity, Email), PostConfirmationInputError> {
    let header = &event.cognito_event_user_pools_header;
    let region = header
        .region
        .as_deref()
        .ok_or(PostConfirmationInputError::MissingField { name: "region" })?;
    let user_pool_id = header
        .user_pool_id
        .as_deref()
        .ok_or(PostConfirmationInputError::MissingField { name: "userPoolId" })?;
    let issuer = CognitoIssuer::try_from(format!(
        "https://cognito-idp.{region}.amazonaws.com/{user_pool_id}"
    ))
    .map_err(|_| PostConfirmationInputError::InvalidIdentity)?;
    let subject = event
        .request
        .user_attributes
        .get("sub")
        .ok_or(PostConfirmationInputError::MissingField { name: "sub" })
        .and_then(|subject| {
            CognitoSubject::try_from(subject.as_str())
                .map_err(|_| PostConfirmationInputError::InvalidIdentity)
        })?;
    let email = event
        .request
        .user_attributes
        .get("email")
        .ok_or(PostConfirmationInputError::MissingField { name: "email" })?
        .as_str()
        .try_into()
        .map_err(|_| PostConfirmationInputError::InvalidEmail)?;

    Ok((CognitoIdentity { issuer, subject }, email))
}

#[cfg(test)]
mod tests {
    use super::{handler, parse_user};
    use application::operation_context::{OperationContext, Principal};
    use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
    use lambda_runtime::{Context, LambdaEvent};
    use serde_email::Email;
    use std::sync::Mutex;
    use user_core::user_id::UserId;
    use user_service::use_cases::{
        RegisterCognitoUserCommand, RegisterCognitoUserError, RegisterCognitoUserResult,
        RegisterCognitoUserUseCase,
    };

    #[derive(Default)]
    struct FakeRegisterCognitoUserUseCase {
        calls: Mutex<Vec<(OperationContext, RegisterCognitoUserCommand)>>,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl RegisterCognitoUserUseCase for FakeRegisterCognitoUserUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: RegisterCognitoUserCommand,
        ) -> Result<RegisterCognitoUserResult, RegisterCognitoUserError> {
            if self.fail {
                return Err(RegisterCognitoUserError::BeginTransactionFailed);
            }
            let result = RegisterCognitoUserResult {
                user_id: UserId::new(),
                email: command.email.clone(),
            };
            let mut calls = match self.calls.lock() {
                Ok(calls) => calls,
                Err(poisoned) => poisoned.into_inner(),
            };
            calls.push((context.clone(), command));
            Ok(result)
        }
    }

    fn event(attributes: serde_json::Value) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
        let payload = match serde_json::from_value(attributes) {
            Ok(payload) => payload,
            Err(error) => panic!("invalid test Cognito event: {error}"),
        };
        let mut context = Context::default();
        context.request_id = "lambda-request-id".to_owned();
        LambdaEvent { payload, context }
    }

    fn post_confirmation_event(
        subject: &str,
        email: &str,
    ) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
        event(serde_json::json!({
            "version": "1",
            "triggerSource": "PostConfirmation_ConfirmSignUp",
            "region": "eu-central-1",
            "userPoolId": "pool-id",
            "userName": "provider-username",
            "callerContext": {},
            "request": {
                "userAttributes": {
                    "sub": subject,
                    "email": email
                },
                "clientMetadata": {}
            },
            "response": {}
        }))
    }

    #[tokio::test]
    async fn should_map_opaque_cognito_identity_to_system_registration_command() {
        let service = FakeRegisterCognitoUserUseCase::default();
        let event = post_confirmation_event("provider|not-a-uuid", "ada@example.com");

        let response = match handler(event, &service).await {
            Ok(response) => response,
            Err(error) => panic!("expected success: {error}"),
        };
        let calls = match service.calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        };

        assert_eq!(
            "provider|not-a-uuid",
            response.request.user_attributes["sub"]
        );
        assert_eq!(1, calls.len());
        assert!(matches!(calls[0].0.principal, Principal::System));
        assert_eq!("lambda-request-id", calls[0].0.request_id.as_str());
        assert_eq!("lambda-request-id", calls[0].0.correlation_id.as_str());
        assert_eq!(
            "https://cognito-idp.eu-central-1.amazonaws.com/pool-id",
            calls[0].1.identity.issuer.as_str()
        );
        assert_eq!("provider|not-a-uuid", calls[0].1.identity.subject.as_str());
        assert_eq!(email("ada@example.com"), calls[0].1.email);
    }

    #[tokio::test]
    async fn should_fail_without_calling_service_when_required_identity_field_is_missing() {
        let service = FakeRegisterCognitoUserUseCase::default();
        let event = event(serde_json::json!({
            "region": "eu-central-1",
            "userPoolId": "pool-id",
            "callerContext": {},
            "request": { "userAttributes": { "email": "ada@example.com" } },
            "response": {}
        }));

        assert!(handler(event, &service).await.is_err());
        let calls = match service.calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(calls.is_empty());
    }

    #[tokio::test]
    async fn should_propagate_service_error_for_cognito_retry() {
        let service = FakeRegisterCognitoUserUseCase {
            fail: true,
            ..Default::default()
        };

        assert!(
            handler(
                post_confirmation_event("provider|opaque-subject", "ada@example.com"),
                &service
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn should_reject_missing_or_invalid_user_attributes() {
        let missing_pool: CognitoEventUserPoolsPostConfirmation =
            match serde_json::from_value(serde_json::json!({
                "region": "eu-central-1",
                "callerContext": {},
                "request": {
                    "userAttributes": { "sub": "opaque", "email": "ada@example.com" }
                },
                "response": {}
            })) {
                Ok(event) => event,
                Err(error) => panic!("invalid test Cognito event: {error}"),
            };
        let invalid_email: CognitoEventUserPoolsPostConfirmation =
            match serde_json::from_value(serde_json::json!({
                "region": "eu-central-1",
                "userPoolId": "pool-id",
                "callerContext": {},
                "request": {
                    "userAttributes": { "sub": "opaque", "email": "invalid" }
                },
                "response": {}
            })) {
                Ok(event) => event,
                Err(error) => panic!("invalid test Cognito event: {error}"),
            };

        assert!(parse_user(&missing_pool).is_err());
        assert!(parse_user(&invalid_email).is_err());
    }

    fn email(value: &str) -> Email {
        match value.try_into() {
            Ok(email) => email,
            Err(error) => panic!("invalid test email: {error}"),
        }
    }
}

use application::transaction::{Transaction, UnitOfWork};
use aws_lambda_events::cognito::CognitoEventUserPoolsPostConfirmation;
use cognito_post_confirmation::handler;
use lambda_runtime::{Context, LambdaEvent};
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::role::UserRole;
use user_core::tier::UserTier;
use user_core::user_id::UserId;
use user_postgres::{
    SqlxCognitoUserIdentityReader, SqlxUserCognitoIdentityRegistryFactory,
    SqlxUserRepositoryFactory,
};
use user_service::ports::{
    CognitoIdentity, CognitoIssuer, CognitoSubject, UserRepository, UserRepositoryFactory,
};
use user_service::use_cases::{
    RegisterCognitoUserHandler, ResolveCognitoUserError, ResolveCognitoUserHandler,
    ResolveCognitoUserRequest, ResolveCognitoUserUseCase,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_register_opaque_cognito_subject_as_independent_default_user() {
    let pool = get_postgres_client().await;
    let service = RegisterCognitoUserHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxUserRepositoryFactory::new(),
        SqlxUserCognitoIdentityRegistryFactory::new(),
    );
    let subject = "provider|tenant:user/42";

    let response = handler(
        post_confirmation_event("eu-central-1", "pool-a", subject, "ada@example.com"),
        &service,
    )
    .await
    .unwrap_or_else(|error| panic!("handler failed: {error}"));
    let user_id = resolve(identity("eu-central-1", "pool-a", subject))
        .await
        .unwrap_or_else(|error| panic!("registered identity did not resolve: {error}"));
    let unit_of_work = SqlxUnitOfWork::new(pool);
    let mut tx = begin(&unit_of_work).await;
    let stored = SqlxUserRepositoryFactory::new()
        .in_transaction(&mut tx)
        .find_by_id(user_id)
        .await
        .unwrap_or_else(|error| panic!("failed to read created user: {error:?}"))
        .unwrap_or_else(|| panic!("created user missing from Postgres"));
    commit(tx).await;

    assert_eq!(subject, response.request.user_attributes["sub"]);
    assert_eq!(7, user_id.as_uuid().get_version_num());
    assert_ne!(subject, user_id.to_string());
    assert_eq!("ada@example.com", stored.value.email().to_string());
    assert_eq!(UserTier::Free, stored.value.account().tier);
    assert_eq!(UserRole::User, stored.value.account().role);
    assert!(stored.value.profile().first_name.is_none());
    assert!(stored.value.preferences().language.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_register_once_when_same_confirmation_runs_concurrently() {
    let pool = get_postgres_client().await;
    let service = RegisterCognitoUserHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxUserRepositoryFactory::new(),
        SqlxUserCognitoIdentityRegistryFactory::new(),
    );
    let first = handler(
        post_confirmation_event(
            "eu-central-1",
            "pool-a",
            "provider|same-subject",
            "ada@example.com",
        ),
        &service,
    );
    let second = handler(
        post_confirmation_event(
            "eu-central-1",
            "pool-a",
            "provider|same-subject",
            "ada@example.com",
        ),
        &service,
    );

    let (first, second) = tokio::join!(first, second);

    assert!(first.is_ok());
    assert!(second.is_ok());
    let user_id = resolve(identity("eu-central-1", "pool-a", "provider|same-subject"))
        .await
        .unwrap_or_else(|error| panic!("registered identity did not resolve: {error}"));
    let (users, identities) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM users), (SELECT count(*) FROM user_cognito_identities)",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count registration rows: {error}"));
    let version = sqlx::query_scalar::<_, i64>("SELECT version FROM users WHERE user_id = $1")
        .bind(user_id.as_uuid())
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read user version: {error}"));

    assert_eq!((1, 1), (users, identities));
    assert_eq!(1, version);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_isolate_same_subject_by_cognito_issuer() {
    let pool = get_postgres_client().await;
    let service = RegisterCognitoUserHandler::new(
        SqlxUnitOfWork::new(pool),
        SqlxUserRepositoryFactory::new(),
        SqlxUserCognitoIdentityRegistryFactory::new(),
    );
    let subject = "provider|shared-subject";

    handler(
        post_confirmation_event("eu-central-1", "pool-a", subject, "ada@example.com"),
        &service,
    )
    .await
    .unwrap_or_else(|error| panic!("first issuer registration failed: {error}"));
    handler(
        post_confirmation_event("eu-central-1", "pool-b", subject, "grace@example.com"),
        &service,
    )
    .await
    .unwrap_or_else(|error| panic!("second issuer registration failed: {error}"));

    let first = resolve(identity("eu-central-1", "pool-a", subject))
        .await
        .unwrap_or_else(|error| panic!("first issuer did not resolve: {error}"));
    let second = resolve(identity("eu-central-1", "pool-b", subject))
        .await
        .unwrap_or_else(|error| panic!("second issuer did not resolve: {error}"));

    assert_ne!(first, second);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_changed_email_for_registered_identity_without_mutation() {
    let pool = get_postgres_client().await;
    let service = RegisterCognitoUserHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxUserRepositoryFactory::new(),
        SqlxUserCognitoIdentityRegistryFactory::new(),
    );
    let cognito_identity = identity("eu-central-1", "pool-a", "provider|replayed-subject");

    handler(
        post_confirmation_event(
            "eu-central-1",
            "pool-a",
            "provider|replayed-subject",
            "ada@example.com",
        ),
        &service,
    )
    .await
    .unwrap_or_else(|error| panic!("initial registration failed: {error}"));
    let original_user_id = resolve(cognito_identity.clone())
        .await
        .unwrap_or_else(|error| panic!("initial identity did not resolve: {error}"));
    let replay = handler(
        post_confirmation_event(
            "eu-central-1",
            "pool-a",
            "provider|replayed-subject",
            "grace@example.com",
        ),
        &service,
    )
    .await;

    assert!(replay.is_err());
    assert_eq!(
        original_user_id,
        resolve(cognito_identity)
            .await
            .unwrap_or_else(|error| panic!("identity changed after conflict: {error}"))
    );
    let emails = sqlx::query_scalar::<_, String>("SELECT email FROM users")
        .fetch_all(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to read users after conflict: {error}"));
    assert_eq!(vec!["ada@example.com"], emails);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_new_identity_for_existing_email_without_orphan_binding() {
    let pool = get_postgres_client().await;
    let service = RegisterCognitoUserHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxUserRepositoryFactory::new(),
        SqlxUserCognitoIdentityRegistryFactory::new(),
    );

    handler(
        post_confirmation_event(
            "eu-central-1",
            "pool-a",
            "provider|first-subject",
            "ada@example.com",
        ),
        &service,
    )
    .await
    .unwrap_or_else(|error| panic!("initial registration failed: {error}"));
    let conflicting_identity = identity("eu-central-1", "pool-a", "provider|second-subject");
    let conflict = handler(
        post_confirmation_event(
            "eu-central-1",
            "pool-a",
            "provider|second-subject",
            "ada@example.com",
        ),
        &service,
    )
    .await;

    assert!(conflict.is_err());
    assert!(matches!(
        resolve(conflicting_identity).await,
        Err(ResolveCognitoUserError::NotFound)
    ));
    let (users, identities) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM users), (SELECT count(*) FROM user_cognito_identities)",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed to count rows after conflict: {error}"));
    assert_eq!((1, 1), (users, identities));
}

fn identity(region: &str, user_pool_id: &str, subject: &str) -> CognitoIdentity {
    CognitoIdentity {
        issuer: CognitoIssuer::try_from(format!(
            "https://cognito-idp.{region}.amazonaws.com/{user_pool_id}"
        ))
        .unwrap_or_else(|error| panic!("invalid test issuer: {error}")),
        subject: CognitoSubject::try_from(subject)
            .unwrap_or_else(|error| panic!("invalid test subject: {error}")),
    }
}

async fn resolve(identity: CognitoIdentity) -> Result<UserId, ResolveCognitoUserError> {
    ResolveCognitoUserHandler::new(SqlxCognitoUserIdentityReader::new(
        get_postgres_client().await,
    ))
    .execute(ResolveCognitoUserRequest { identity })
    .await
    .map(|result| result.user_id)
}

fn post_confirmation_event(
    region: &str,
    user_pool_id: &str,
    subject: &str,
    email: &str,
) -> LambdaEvent<CognitoEventUserPoolsPostConfirmation> {
    let payload = serde_json::from_value(serde_json::json!({
        "version": "1",
        "triggerSource": "PostConfirmation_ConfirmSignUp",
        "region": region,
        "userPoolId": user_pool_id,
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
    .unwrap_or_else(|error| panic!("invalid test Cognito event: {error}"));
    let mut context = Context::default();
    context.request_id = "lambda-request-id".to_owned();

    LambdaEvent { payload, context }
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> SqlxTransaction {
    unit_of_work
        .begin()
        .await
        .unwrap_or_else(|error| panic!("failed to begin transaction: {error}"))
}

async fn commit(tx: SqlxTransaction) {
    tx.commit()
        .await
        .unwrap_or_else(|error| panic!("failed to commit transaction: {error}"));
}

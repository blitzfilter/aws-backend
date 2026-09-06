mod mapping;
mod readers;
mod repositories;

pub use readers::{
    SqlxListingSourceAuthorization, SqlxPartnershipApplicationReaderFactory,
    SqlxPartnershipDetailsReaderFactory, SqlxPartnershipSearchReaderFactory,
};
pub use repositories::{
    SqlxListingSourceGrantRepositoryFactory, SqlxPartnershipApplicationRepositoryFactory,
    SqlxPartnershipRepositoryFactory,
};

#[cfg(test)]
mod tests {
    use super::*;
    use application::{
        operation_context::{CorrelationId, OperationContext, Principal, RequestId},
        transaction::{Transaction, UnitOfWork},
    };
    use listing_source_postgres::SqlxListingSourceRepositoryFactory;
    use notification_service::ports::notification_creator::{
        NewNotification, NotificationCreationError, NotificationCreationOutcome,
        NotificationCreator, NotificationCreatorFactory,
    };
    use partnership_core::partnership_application_id::PartnershipApplicationId;
    use partnership_service::{
        ports::{
            ListingSourceAuthorization, PartnershipApplicationRepository,
            PartnershipApplicationRepositoryFactory,
        },
        use_cases::commands::approve_partnership_application::{
            ApprovePartnershipApplicationCommand, ApprovePartnershipApplicationHandler,
            ApprovePartnershipApplicationUseCase,
        },
    };
    use party_postgres::SqlxPartyRepositoryFactory;
    use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
    use serde_json::json;
    use sqlx::PgPool;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
    use user_core::user_id::UserId;
    use user_postgres::SqlxUserAdminReaderFactory;

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    fn system_context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    #[derive(Clone, Default)]
    struct TestNotificationCreatorFactory {
        created_count: Arc<AtomicUsize>,
    }

    impl TestNotificationCreatorFactory {
        fn created_count(&self) -> usize {
            self.created_count.load(Ordering::SeqCst)
        }
    }

    struct TestNotificationCreator {
        created_count: Arc<AtomicUsize>,
    }

    impl NotificationCreatorFactory<SqlxTransaction> for TestNotificationCreatorFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut SqlxTransaction,
        ) -> impl NotificationCreator + 'tx {
            TestNotificationCreator {
                created_count: Arc::clone(&self.created_count),
            }
        }
    }

    #[async_trait::async_trait]
    impl NotificationCreator for TestNotificationCreator {
        async fn create_many(
            &mut self,
            notifications: &[NewNotification],
        ) -> Result<Vec<NotificationCreationOutcome>, NotificationCreationError> {
            self.created_count
                .fetch_add(notifications.len(), Ordering::SeqCst);
            Ok(notifications
                .iter()
                .map(|_| NotificationCreationOutcome::Duplicate)
                .collect())
        }
    }

    fn handler(
        pool: PgPool,
    ) -> ApprovePartnershipApplicationHandler<
        SqlxUnitOfWork,
        SqlxPartnershipApplicationRepositoryFactory,
        SqlxPartyRepositoryFactory,
        SqlxListingSourceRepositoryFactory,
        SqlxPartnershipRepositoryFactory,
        SqlxPartnershipRepositoryFactory,
        SqlxListingSourceGrantRepositoryFactory,
        SqlxUserAdminReaderFactory,
        TestNotificationCreatorFactory,
    > {
        handler_with_notifications(pool, TestNotificationCreatorFactory::default())
    }

    fn handler_with_notifications(
        pool: PgPool,
        notifications: TestNotificationCreatorFactory,
    ) -> ApprovePartnershipApplicationHandler<
        SqlxUnitOfWork,
        SqlxPartnershipApplicationRepositoryFactory,
        SqlxPartyRepositoryFactory,
        SqlxListingSourceRepositoryFactory,
        SqlxPartnershipRepositoryFactory,
        SqlxPartnershipRepositoryFactory,
        SqlxListingSourceGrantRepositoryFactory,
        SqlxUserAdminReaderFactory,
        TestNotificationCreatorFactory,
    > {
        ApprovePartnershipApplicationHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxPartnershipApplicationRepositoryFactory::new(),
            SqlxPartyRepositoryFactory::new(),
            SqlxListingSourceRepositoryFactory::new(),
            SqlxPartnershipRepositoryFactory::new(),
            SqlxPartnershipRepositoryFactory::new(),
            SqlxListingSourceGrantRepositoryFactory::new(),
            SqlxUserAdminReaderFactory::new(),
            notifications,
        )
    }

    async fn seed_user(pool: &PgPool) -> UserId {
        let user_id = UserId::new();
        let result = sqlx::query(
            "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')",
        )
        .bind(uuid::Uuid::from(user_id))
        .bind(format!("{user_id}@example.test"))
        .execute(pool)
        .await;
        assert!(result.is_ok());
        user_id
    }

    async fn seed_application(
        pool: &PgPool,
        applicant_user_id: UserId,
        state: &str,
        proposal: serde_json::Value,
    ) -> PartnershipApplicationId {
        let application_id = PartnershipApplicationId::new();
        let result = sqlx::query(
            "INSERT INTO partnership_applications (partnership_application_id, applicant_user_id, business_state, proposal) VALUES ($1, $2, $3, $4)",
        )
        .bind(uuid::Uuid::from(application_id))
        .bind(uuid::Uuid::from(applicant_user_id))
        .bind(state)
        .bind(proposal)
        .execute(pool)
        .await;
        assert!(result.is_ok());
        application_id
    }

    async fn count(pool: &PgPool, table: &str) -> i64 {
        let result = match table {
            "parties" => {
                sqlx::query_scalar("SELECT count(*) FROM parties")
                    .fetch_one(pool)
                    .await
            }
            "listing_sources" => {
                sqlx::query_scalar("SELECT count(*) FROM listing_sources")
                    .fetch_one(pool)
                    .await
            }
            "partnerships" => {
                sqlx::query_scalar("SELECT count(*) FROM partnerships")
                    .fetch_one(pool)
                    .await
            }
            "partnership_members" => {
                sqlx::query_scalar("SELECT count(*) FROM partnership_members")
                    .fetch_one(pool)
                    .await
            }
            "partnership_listing_source_grants" => {
                sqlx::query_scalar("SELECT count(*) FROM partnership_listing_source_grants")
                    .fetch_one(pool)
                    .await
            }
            _ => panic!("unsupported count table: {table}"),
        };
        match result {
            Ok(value) => value,
            Err(error) => panic!("count {table}: {error}"),
        }
    }

    async fn count_members_for_user(pool: &PgPool, user_id: UserId) -> i64 {
        match sqlx::query_scalar("SELECT count(*) FROM partnership_members WHERE user_id = $1")
            .bind(uuid::Uuid::from(user_id))
            .fetch_one(pool)
            .await
        {
            Ok(value) => value,
            Err(error) => panic!("count partnership members for user: {error}"),
        }
    }

    async fn count_grants_for_source(pool: &PgPool, listing_source_id: uuid::Uuid) -> i64 {
        match sqlx::query_scalar(
            "SELECT count(*) FROM partnership_listing_source_grants WHERE listing_source_id = $1",
        )
        .bind(listing_source_id)
        .fetch_one(pool)
        .await
        {
            Ok(value) => value,
            Err(error) => panic!("count listing source grants: {error}"),
        }
    }

    async fn application_state(pool: &PgPool, application_id: PartnershipApplicationId) -> String {
        match sqlx::query_scalar::<_, String>(
            "SELECT business_state FROM partnership_applications WHERE partnership_application_id = $1",
        )
        .bind(uuid::Uuid::from(application_id))
        .fetch_one(pool)
        .await
        {
            Ok(state) => state,
            Err(error) => panic!("read application state: {error}"),
        }
    }

    fn existing_source(source_id: uuid::Uuid) -> serde_json::Value {
        json!({
            "type": "EXISTING_LISTING_SOURCE",
            "listing_source_id": source_id,
        })
    }

    fn proposed_source() -> serde_json::Value {
        json!({
            "type": "PROPOSED_LISTING_SOURCE",
            "party": { "name": "Northwind Antiques", "phone": null, "email": null },
            "listing_source": {
                "name": "Northwind Source",
                "url": null,
                "image": null,
                "requested_ingestion_methods": ["PARTNER_API"]
            }
        })
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_commit_proposed_source_approval_and_replay_without_duplicate_members_or_grants()
    {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let application_id = seed_application(&pool, user_id, "IN_REVIEW", proposed_source()).await;
        let handler = handler(pool.clone());

        let first = handler
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await
            .unwrap_or_else(|error| panic!("approve application: {error}"));
        let second = handler
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await
            .unwrap_or_else(|error| panic!("replay approved application: {error}"));

        assert_eq!(first.partnership_id, second.partnership_id);
        assert_eq!(first.listing_source_id, second.listing_source_id);
        assert!(second.partnership_id.is_some());
        assert!(second.listing_source_id.is_some());
        assert_eq!("APPROVED", application_state(&pool, application_id).await);
        assert_eq!(1, count(&pool, "parties").await);
        assert_eq!(1, count(&pool, "listing_sources").await);
        assert_eq!(1, count(&pool, "partnerships").await);
        assert_eq!(1, count(&pool, "partnership_members").await);
        assert_eq!(1, count(&pool, "partnership_listing_source_grants").await);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_block_locked_application_lookup_until_lock_released() {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let application_id = seed_application(&pool, user_id, "IN_REVIEW", proposed_source()).await;
        let factory = SqlxPartnershipApplicationRepositoryFactory::new();
        let mut locking_transaction = match SqlxUnitOfWork::new(pool.clone()).begin().await {
            Ok(transaction) => transaction,
            Err(error) => panic!("begin locking transaction: {error}"),
        };
        let locked = match factory
            .in_transaction(&mut locking_transaction)
            .find_by_id_for_update(application_id)
            .await
        {
            Ok(application) => application,
            Err(error) => panic!("lock partnership application: {error}"),
        };
        assert!(locked.is_some());

        let lookup_pool = pool.clone();
        let mut blocked_lookup = Box::pin(async move {
            let mut transaction = match SqlxUnitOfWork::new(lookup_pool).begin().await {
                Ok(transaction) => transaction,
                Err(error) => panic!("begin blocked lookup transaction: {error}"),
            };
            let application = match SqlxPartnershipApplicationRepositoryFactory::new()
                .in_transaction(&mut transaction)
                .find_by_id_for_update(application_id)
                .await
            {
                Ok(application) => application,
                Err(error) => panic!("read locked partnership application: {error}"),
            };
            if let Err(error) = transaction.commit().await {
                panic!("commit blocked lookup transaction: {error}");
            }
            application
        });

        tokio::select! {
            application = &mut blocked_lookup => panic!("locked lookup completed early: {application:?}"),
            () = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        if let Err(error) = locking_transaction.commit().await {
            panic!("commit locking transaction: {error}");
        }

        assert!(blocked_lookup.await.is_some());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_replay_concurrent_approval_with_stable_result_ids() {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let application_id = seed_application(&pool, user_id, "IN_REVIEW", proposed_source()).await;
        let first_handler = handler(pool.clone());
        let second_handler = handler(pool.clone());
        let command = ApprovePartnershipApplicationCommand { application_id };
        let first_context = system_context();
        let second_context = system_context();

        let (first, second) = tokio::join!(
            first_handler.execute(&first_context, command.clone()),
            second_handler.execute(&second_context, command),
        );
        let first = first.unwrap_or_else(|error| panic!("first concurrent approval: {error}"));
        let second = second.unwrap_or_else(|error| panic!("second concurrent approval: {error}"));

        assert_eq!(first.partnership_id, second.partnership_id);
        assert_eq!(first.listing_source_id, second.listing_source_id);
        assert_eq!(1, count(&pool, "parties").await);
        assert_eq!(1, count(&pool, "listing_sources").await);
        assert_eq!(1, count(&pool, "partnerships").await);
        assert_eq!(1, count(&pool, "partnership_members").await);
        assert_eq!(1, count(&pool, "partnership_listing_source_grants").await);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_approve_concurrent_existing_source_applications_with_one_partnership() {
        let pool = get_postgres_client().await;
        let first_user_id = seed_user(&pool).await;
        let second_user_id = seed_user(&pool).await;
        let party_id = uuid::Uuid::new_v4();
        let source_id = uuid::Uuid::new_v4();
        let party = sqlx::query(
            "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Concurrent Operator')",
        )
        .bind(party_id)
        .bind(format!("concurrent-operator-{party_id}"))
        .execute(&pool)
        .await;
        assert!(party.is_ok());
        let source = sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Concurrent Source', $3)",
        )
        .bind(source_id)
        .bind(format!("concurrent-source-{source_id}"))
        .bind(party_id)
        .execute(&pool)
        .await;
        assert!(source.is_ok());
        let first_application_id = seed_application(
            &pool,
            first_user_id,
            "IN_REVIEW",
            existing_source(source_id),
        )
        .await;
        let second_application_id = seed_application(
            &pool,
            second_user_id,
            "IN_REVIEW",
            existing_source(source_id),
        )
        .await;
        let first_notifications = TestNotificationCreatorFactory::default();
        let second_notifications = TestNotificationCreatorFactory::default();
        let first_handler = handler_with_notifications(pool.clone(), first_notifications.clone());
        let second_handler = handler_with_notifications(pool.clone(), second_notifications.clone());
        let first_context = system_context();
        let second_context = system_context();

        let (first, second) = tokio::join!(
            first_handler.execute(
                &first_context,
                ApprovePartnershipApplicationCommand {
                    application_id: first_application_id,
                },
            ),
            second_handler.execute(
                &second_context,
                ApprovePartnershipApplicationCommand {
                    application_id: second_application_id,
                },
            ),
        );
        let first = first.unwrap_or_else(|error| panic!("first concurrent approval: {error}"));
        let second = second.unwrap_or_else(|error| panic!("second concurrent approval: {error}"));

        assert_eq!(first.partnership_id, second.partnership_id);
        assert!(first.partnership_id.is_some());
        assert_eq!(
            Some(listing_source_core::ListingSourceId::from(source_id)),
            first.listing_source_id
        );
        assert_eq!(
            Some(listing_source_core::ListingSourceId::from(source_id)),
            second.listing_source_id
        );
        assert_eq!(
            "APPROVED",
            application_state(&pool, first_application_id).await
        );
        assert_eq!(
            "APPROVED",
            application_state(&pool, second_application_id).await
        );
        assert_eq!(1, count(&pool, "parties").await);
        assert_eq!(1, count(&pool, "listing_sources").await);
        assert_eq!(1, count(&pool, "partnerships").await);
        assert_eq!(2, count(&pool, "partnership_members").await);
        assert_eq!(1, count_members_for_user(&pool, first_user_id).await);
        assert_eq!(1, count_members_for_user(&pool, second_user_id).await);
        assert_eq!(1, count(&pool, "partnership_listing_source_grants").await);
        assert_eq!(1, count_grants_for_source(&pool, source_id).await);
        assert_eq!(1, first_notifications.created_count());
        assert_eq!(1, second_notifications.created_count());
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_approve_existing_source_and_expose_granted_source_authorization() {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let party_id = uuid::Uuid::new_v4();
        let source_id = uuid::Uuid::new_v4();
        let party = sqlx::query(
            "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, 'existing-operator', 'Existing Operator')",
        )
        .bind(party_id)
        .execute(&pool)
        .await;
        assert!(party.is_ok());
        let source = sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, 'existing-source', 'Existing Source', $2)",
        )
        .bind(source_id)
        .bind(party_id)
        .execute(&pool)
        .await;
        assert!(source.is_ok());
        let application_id = seed_application(
            &pool,
            user_id,
            "IN_REVIEW",
            json!({
                "type": "EXISTING_LISTING_SOURCE",
                "listing_source_id": source_id,
            }),
        )
        .await;

        let result = handler(pool.clone())
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;
        assert!(result.is_ok());
        let authorization = SqlxListingSourceAuthorization::new(pool.clone());
        let source_id = listing_source_core::ListingSourceId::from(source_id);
        assert!(matches!(
            authorization.can_write_source(user_id, source_id).await,
            Ok(true)
        ));
        let administered = authorization.list_sources_user_administers(user_id).await;
        assert!(matches!(administered, Ok(sources) if sources.len() == 1));
        assert_eq!(1, count(&pool, "parties").await);
        assert_eq!(1, count(&pool, "listing_sources").await);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_approve_proposed_source_when_another_party_has_the_same_name() {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let existing_party = sqlx::query(
            "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, 'northwind-antiques-existing', 'Northwind Antiques')",
        )
        .bind(uuid::Uuid::new_v4())
        .execute(&pool)
        .await;
        assert!(existing_party.is_ok());
        let application_id = seed_application(&pool, user_id, "IN_REVIEW", proposed_source()).await;

        let result = handler(pool.clone())
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(result.is_ok());
        assert_eq!("APPROVED", application_state(&pool, application_id).await);
        assert_eq!(2, count(&pool, "parties").await);
        assert_eq!(1, count(&pool, "listing_sources").await);
        assert_eq!(1, count(&pool, "partnerships").await);
        assert_eq!(1, count(&pool, "partnership_members").await);
        assert_eq!(1, count(&pool, "partnership_listing_source_grants").await);
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_leave_submitted_application_unchanged_when_approval_is_invalid() {
        let pool = get_postgres_client().await;
        let user_id = seed_user(&pool).await;
        let application_id = seed_application(&pool, user_id, "SUBMITTED", proposed_source()).await;

        let result = handler(pool.clone())
            .execute(
                &system_context(),
                ApprovePartnershipApplicationCommand { application_id },
            )
            .await;

        assert!(matches!(
            result,
            Err(partnership_service::use_cases::commands::approve_partnership_application::ApprovePartnershipApplicationError::ApplicationNotApprovable)
        ));
        assert_eq!("SUBMITTED", application_state(&pool, application_id).await);
        assert_eq!(0, count(&pool, "parties").await);
    }
}

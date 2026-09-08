use async_trait::async_trait;
use aws_lambda_events::eventbridge::EventBridgeEvent;
use fxrate_lambda::handler;
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::{
    CaptureFxRateSnapshotHandler,
    ports::{FxRateQuote, FxRateQuoteProvider, FxRateQuoteProviderError, FxRateQuoteSet},
};
use lambda_runtime::{Context, LambdaEvent};
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use serde_json::Value;
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

struct Quotes {
    complete: bool,
}

#[async_trait]
impl FxRateQuoteProvider for Quotes {
    async fn fetch_eur_quotes(&self) -> Result<FxRateQuoteSet, FxRateQuoteProviderError> {
        let mut quotes = Currency::iter()
            .filter(|currency| *currency != Currency::Eur)
            .map(|currency| FxRateQuote {
                currency,
                units_per_eur: 1_250_000,
            })
            .collect::<Vec<_>>();
        if !self.complete {
            quotes.pop();
        }
        Ok(FxRateQuoteSet {
            base: Currency::Eur,
            quotes,
        })
    }
}

fn event(id: &str) -> LambdaEvent<EventBridgeEvent<Value>> {
    let mut payload = EventBridgeEvent::default();
    payload.id = Some(id.to_owned());
    payload.source = "aura.test.fx-rate-schedule".to_owned();
    LambdaEvent::new(payload, Context::default())
}

fn event_without_id() -> LambdaEvent<EventBridgeEvent<Value>> {
    LambdaEvent::new(EventBridgeEvent::default(), Context::default())
}

fn snapshots(
    pool: sqlx::PgPool,
    complete: bool,
) -> CaptureFxRateSnapshotHandler<Quotes, SqlxUnitOfWork, SqlxFxRateSnapshotRepositoryFactory> {
    CaptureFxRateSnapshotHandler::new(
        Quotes { complete },
        SqlxUnitOfWork::new(pool),
        SqlxFxRateSnapshotRepositoryFactory::new(),
    )
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_capture_complete_fx_snapshot_from_scheduled_event() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        handler(event("scheduled-capture"), &snapshots(pool.clone(), true)).await?;
        let (snapshots, conversions): (i64, i64) = sqlx::query_as(
            r#"
            SELECT
                (SELECT count(*) FROM fx_rates WHERE source_event_id = 'scheduled-capture'),
                (SELECT count(*) FROM fx_rate_quotes c
                 JOIN fx_rates r ON r.fx_rate_id = c.fx_rate_id
                 WHERE r.source_event_id = 'scheduled-capture')
            "#,
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(1, snapshots);
        let expected_quotes = i64::try_from(Currency::iter().count())?;
        assert_eq!(expected_quotes, conversions);
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "FX lambda integration failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_not_persist_partial_snapshot_and_should_deduplicate_retry() {
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        let pool = get_postgres_client().await;
        let missing_id = handler(event_without_id(), &snapshots(pool.clone(), true)).await;
        assert!(missing_id.is_err());

        let incomplete = handler(
            event("scheduled-incomplete"),
            &snapshots(pool.clone(), false),
        )
        .await;
        assert!(incomplete.is_err());
        let incomplete_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fx_rates WHERE source_event_id = 'scheduled-incomplete'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(0, incomplete_count);

        let service = snapshots(pool.clone(), true);
        handler(event("scheduled-retry"), &service).await?;
        handler(event("scheduled-retry"), &service).await?;
        let retry_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fx_rates WHERE source_event_id = 'scheduled-retry'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(1, retry_count);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "FX lambda failure/idempotency integration failed: {result:?}"
    );
}

use crawler::local_db::{CrawlerSchemaError, ServerDatabaseConfig, verify_crawler_schema};
use platform_postgres::{PostgresConnectError, PostgresSchemaError, verify_business_schema};
use sqlx::PgPool;
use std::future::Future;
use std::time::Duration;
use tokio::time::{Instant, timeout, timeout_at};

const OVERALL_TIMEOUT: Duration = Duration::from_secs(60);
const DEPENDENCY_TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub(super) enum PreflightError {
    #[error("crawler database connection check failed: {0}")]
    CrawlerConnect(#[source] PostgresConnectError),
    #[error("business database connection check failed: {0}")]
    BusinessConnect(#[source] PostgresConnectError),
    #[error("crawler database schema check failed: {0}")]
    CrawlerSchema(#[from] CrawlerSchemaError),
    #[error("business database schema check failed: {0}")]
    BusinessSchema(#[from] PostgresSchemaError),
    #[error("crawler database preflight timed out")]
    CrawlerTimeout,
    #[error("business database preflight timed out")]
    BusinessTimeout,
    #[error("database preflight overall deadline exceeded")]
    Timeout,
    #[error("database preflight cleanup timed out; safe cleanup not confirmed")]
    CleanupTimeout {
        #[source]
        check_error: Option<Box<PreflightError>>,
    },
}

pub(super) async fn run(config: &ServerDatabaseConfig) -> Result<(), PreflightError> {
    let started = Instant::now();
    // Lazy creation retains BOTH handles before the first cancellable connection attempt.
    // Use shared validated TLS/options and pool caps; no raw URLs or ambient fallback here.
    let crawler = config
        .crawler
        .pool_options()
        .connect_lazy_with(config.crawler.connect_options());
    let business = config
        .business
        .pool_options()
        .connect_lazy_with(config.business.connect_options());
    let checks = async {
        // Do not try_join: failure of one dependency must not discard the other's ownership.
        let (crawler_result, business_result) =
            tokio::join!(check_crawler(&crawler), check_business(&business));
        crawler_result.and(business_result)
    };
    finish_with_cleanup(started, checks, async {
        tokio::join!(crawler.close(), business.close());
    })
    .await
}

async fn check_crawler(pool: &PgPool) -> Result<(), PreflightError> {
    timeout(DEPENDENCY_TIMEOUT, async {
        let connection = pool
            .acquire()
            .await
            .map_err(|error| PreflightError::CrawlerConnect(error.into()))?;
        drop(connection);
        verify_crawler_schema(pool).await?;
        Ok(())
    })
    .await
    .map_err(|_| PreflightError::CrawlerTimeout)?
}

async fn check_business(pool: &PgPool) -> Result<(), PreflightError> {
    timeout(DEPENDENCY_TIMEOUT, async {
        let connection = pool
            .acquire()
            .await
            .map_err(|error| PreflightError::BusinessConnect(error.into()))?;
        drop(connection);
        verify_business_schema(pool).await?;
        Ok(())
    })
    .await
    .map_err(|_| PreflightError::BusinessTimeout)?
}

async fn finish_with_cleanup(
    started: Instant,
    checks: impl Future<Output = Result<(), PreflightError>>,
    cleanup: impl Future<Output = ()>,
) -> Result<(), PreflightError> {
    // Reserve cleanup and Tokio teardown inside the total 60-second budget.
    let close_deadline = started + OVERALL_TIMEOUT - RUNTIME_SHUTDOWN_TIMEOUT;
    let check_deadline = close_deadline - CLEANUP_TIMEOUT;
    let result = timeout_at(check_deadline, checks)
        .await
        .unwrap_or(Err(PreflightError::Timeout));
    let cleanup_deadline = close_deadline.min(Instant::now() + CLEANUP_TIMEOUT);
    if timeout_at(cleanup_deadline, cleanup).await.is_err() {
        return Err(PreflightError::CleanupTimeout {
            check_error: result.err().map(Box::new),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::future::pending;

    // Deadline/ownership tests only, not PostgreSQL or read-only-schema proof.
    #[tokio::test(start_paused = true)]
    async fn should_cleanup_when_checks_fail() {
        let closed = Cell::new(false);
        let result = finish_with_cleanup(
            Instant::now(),
            async { Err(PreflightError::CrawlerTimeout) },
            async { closed.set(true) },
        )
        .await;
        assert!(matches!(result, Err(PreflightError::CrawlerTimeout)));
        assert!(closed.get());
    }

    #[tokio::test(start_paused = true)]
    async fn should_cleanup_and_fail_when_overall_checks_time_out() {
        let started = Instant::now();
        let closed = Cell::new(false);
        let result = finish_with_cleanup(started, pending(), async { closed.set(true) }).await;
        assert!(matches!(result, Err(PreflightError::Timeout)));
        assert!(closed.get());
        assert_eq!(started.elapsed(), Duration::from_secs(54));
    }

    #[tokio::test(start_paused = true)]
    async fn should_fail_when_successful_checks_cannot_confirm_cleanup() {
        let started = Instant::now();
        let result = finish_with_cleanup(started, async { Ok(()) }, pending()).await;
        assert!(matches!(
            result,
            Err(PreflightError::CleanupTimeout { check_error: None })
        ));
        assert_eq!(started.elapsed(), CLEANUP_TIMEOUT);
    }

    #[tokio::test(start_paused = true)]
    async fn should_wait_for_both_cleanup_futures_before_success() {
        let first = Cell::new(false);
        let second = Cell::new(false);
        let started = Instant::now();
        let result = finish_with_cleanup(started, async { Ok(()) }, async {
            tokio::join!(
                async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    first.set(true);
                },
                async {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    second.set(true);
                },
            );
        })
        .await;
        assert!(result.is_ok());
        assert!(first.get() && second.get());
        assert_eq!(started.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn should_attempt_other_cleanup_when_one_never_finishes() {
        let second = Cell::new(false);
        let result = finish_with_cleanup(Instant::now(), async { Ok(()) }, async {
            tokio::join!(pending::<()>(), async { second.set(true) });
        })
        .await;
        assert!(matches!(result, Err(PreflightError::CleanupTimeout { .. })));
        assert!(second.get());
    }

    #[tokio::test(start_paused = true)]
    async fn should_reserve_cleanup_inside_overall_budget_and_preserve_check_failure() {
        let started = Instant::now();
        let result = finish_with_cleanup(started, pending(), pending()).await;
        assert!(matches!(
            result,
            Err(PreflightError::CleanupTimeout {
                check_error: Some(_)
            })
        ));
        assert_eq!(
            started.elapsed() + RUNTIME_SHUTDOWN_TIMEOUT,
            OVERALL_TIMEOUT
        );
    }

    #[test]
    fn should_redact_entire_database_error_chain() {
        const CANARY: &str = "private-provider-body-password";
        let error = PreflightError::CleanupTimeout {
            check_error: Some(Box::new(PreflightError::CrawlerConnect(
                sqlx::Error::Protocol(CANARY.into()).into(),
            ))),
        };
        let mut current: Option<&dyn std::error::Error> = Some(&error);
        let mut depth = 0;
        while let Some(error) = current {
            assert!(!format!("{error} {error:?} {error:#?}").contains(CANARY));
            current = error.source();
            depth += 1;
        }
        assert_eq!(depth, 4);
    }
}

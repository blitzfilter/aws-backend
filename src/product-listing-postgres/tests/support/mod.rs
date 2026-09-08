use std::{future::Future, pin::Pin, time::Duration};

/// Observe a real database lock wait, not merely a task that has not been scheduled yet.
pub async fn assert_blocked<F: Future>(
    pool: &sqlx::PgPool,
    blocker_pid: i32,
    waiting_count: i64,
    work: Pin<&mut F>,
) -> Result<(), Box<dyn std::error::Error>> {
    let observe = async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                r#"
                                WITH RECURSIVE blocked(pid) AS (
                                    SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))
                                    UNION
                                    SELECT activity.pid FROM pg_stat_activity activity
                                    JOIN blocked ON blocked.pid = ANY(pg_blocking_pids(activity.pid))
                                )
                                SELECT count(*) FROM blocked
                                "#,
            )
            .bind(blocker_pid)
            .fetch_one(pool)
            .await?;
            if blocked >= waiting_count {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::select! {
        biased;
        _ = work => Err(std::io::Error::other("work finished before the owning transaction released its lock").into()),
        observed = tokio::time::timeout(Duration::from_secs(10), observe) => {
            observed??;
            Ok(())
        }
    }
}

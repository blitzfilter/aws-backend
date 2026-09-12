use aura_historia_worker::{WorkerOpenSearchConfig, WorkerStartupConfig, queue::SqsQueue};
use std::time::Duration;

pub(super) struct Prepared {
    pub(super) pool: sqlx::PgPool,
    pub(super) queue: SqsQueue,
}

/// No runtime composition, service handler, reconciliation hook, or provider auth here.
/// The schema owner verifies only; workers never run migrations.
pub(super) async fn check(startup: &WorkerStartupConfig) -> Result<Prepared, super::MainError> {
    let pool = startup.postgres().connect().await?;
    let result = async {
        platform_postgres::verify_business_schema(&pool).await?;
        let queue = SqsQueue::from_config(startup.queue().clone()).await?;
        if let Some(search) = startup.opensearch() {
            check_opensearch(search).await?;
        }
        Ok(queue)
    }
    .await;
    match result {
        Ok(queue) => Ok(Prepared { pool, queue }),
        Err(error) => {
            pool.close().await;
            Err(error)
        }
    }
}

async fn check_opensearch(config: &WorkerOpenSearchConfig) -> Result<(), super::MainError> {
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(5))
            .build()?;
        let mut request = client.get(config.endpoint().clone());
        if let Some((username, password)) = config.basic_auth() {
            request = request.basic_auth(username, Some(password));
        }
        let mut response = request.send().await?;
        if !response.status().is_success() {
            return Ok(false);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > 64 * 1024 {
                return Ok(false);
            }
            body.extend_from_slice(&chunk);
        }
        Ok::<bool, reqwest::Error>(compatible_opensearch(&body))
    })
    .await;
    // Never retain/format remote payloads, URLs or reqwest's provider error chain.
    match result {
        Ok(Ok(true)) => Ok(()),
        _ => Err(super::MainError::OpenSearchCompatibility),
    }
}

fn compatible_opensearch(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    if value["version"]["distribution"] != "opensearch" {
        return false;
    }
    let Some(number) = value["version"]["number"].as_str() else {
        return false;
    };
    let parts = number.split('.').collect::<Vec<_>>();
    // Current infrastructure baseline is OpenSearch 3.1; no untested future major.
    parts.len() == 3
        && parts[0] == "3"
        && parts[1].parse::<u32>().is_ok_and(|minor| minor >= 1)
        && parts[2].parse::<u32>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_reject_wrong_engine_unknown_major_and_malformed_endpoint_identity() {
        for (distribution, number, valid) in [
            ("opensearch", "3.1.0", true),
            ("elasticsearch", "3.1.0", false),
            ("opensearch", "2.19.0", false),
            ("opensearch", "4.0.0", false),
            ("opensearch", "3", false),
            ("opensearch", "3.1.bad", false),
        ] {
            let body =
                serde_json::json!({"version": {"distribution":distribution, "number":number}})
                    .to_string();
            assert_eq!(valid, compatible_opensearch(body.as_bytes()));
        }
        assert!(!compatible_opensearch(b"{}"));
        assert!(!compatible_opensearch(b"not json"));
    }
}

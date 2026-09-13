use super::{Code, Failure, Target};
use crawler::local_db::parse_postgres_environment;
use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError, PostgresTlsConfig};
use std::{env::VarError, net::IpAddr};

pub(super) fn load(
    target: Target,
    initialize: bool,
    get: impl FnMut(&'static str) -> Result<String, VarError>,
) -> Result<PostgresPoolConfig, Failure> {
    if !cfg!(unix) {
        return Err(Failure::new(Code::UnsupportedPlatform));
    }
    parse_postgres_environment(get, |get| {
        let stage = get("STAGE").ok_or(PostgresPoolConfigError::MissingInput("STAGE"))?;
        // This local executable deliberately supports no real stage, including verification.
        if !matches!(stage.as_str(), "local" | "ephemeral" | "test") {
            return Err(Failure::new(Code::UnsupportedStage));
        }
        let tls = PostgresTlsConfig::from_lookup("crawler-bootstrap-local", |key| {
            if key == "STAGE" {
                Some(stage.clone())
            } else {
                get(key)
            }
        })?;
        let key = match target {
            Target::Business => "BUSINESS_DATABASE_URL",
            Target::Crawler => "LOCAL_DB_URL",
        };
        let url = get(key).ok_or(PostgresPoolConfigError::MissingInput(key))?;
        let config = PostgresPoolConfig::from_url(&url, 1, tls)?;
        if initialize && !is_local_endpoint(config.host()) {
            return Err(Failure::new(Code::NonLocalEndpoint));
        }
        Ok(config)
    })
}

fn is_local_endpoint(host: &str) -> bool {
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

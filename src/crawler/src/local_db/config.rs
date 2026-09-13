use super::{LocalDatabaseError, database_url};
use platform_postgres::{PostgresPoolConfig, PostgresPoolConfigError, PostgresTlsConfig};
use std::env::VarError;

/// Adapts a root-owned environment lookup without replacing invalid Unicode or hiding presence.
/// Only run configuration parsing in `parse`; validate its result before any runtime side effects.
pub fn parse_postgres_environment<T, E>(
    mut get: impl FnMut(&'static str) -> Result<String, VarError>,
    parse: impl FnOnce(&mut dyn FnMut(&'static str) -> Option<String>) -> Result<T, E>,
) -> Result<T, E>
where
    E: From<PostgresPoolConfigError>,
{
    let mut invalid_input = None;
    let result = parse(&mut |key| match get(key) {
        Ok(value) => Some(value),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(_)) => {
            invalid_input.get_or_insert(key);
            // Preserve presence for the shared infallible lookup, then return the typed error.
            Some(String::new())
        }
    });
    match invalid_input {
        Some(key) => Err(PostgresPoolConfigError::InvalidInput(key).into()),
        None => result,
    }
}

/// Server startup only connects to explicitly configured, already provisioned databases.
#[derive(Debug)]
pub struct ServerDatabaseConfig {
    pub crawler: PostgresPoolConfig,
    pub business: PostgresPoolConfig,
}

impl ServerDatabaseConfig {
    pub fn from_lookup(
        crawler_max_connections: u32,
        business_max_connections: u32,
        mut get: impl FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, PostgresPoolConfigError> {
        let tls = PostgresTlsConfig::from_lookup("crawler-server", &mut get)?;
        let crawler_url =
            get("LOCAL_DB_URL").ok_or(PostgresPoolConfigError::MissingInput("LOCAL_DB_URL"))?;
        let business_url = get("BUSINESS_DATABASE_URL").ok_or(
            PostgresPoolConfigError::MissingInput("BUSINESS_DATABASE_URL"),
        )?;
        Ok(Self {
            crawler: PostgresPoolConfig::from_url(
                &crawler_url,
                crawler_max_connections,
                tls.clone(),
            )?,
            business: PostgresPoolConfig::from_url(&business_url, business_max_connections, tls)?,
        })
    }
}

/// Capability required by local URL helpers, Docker bootstrap, and demo migrations.
/// No constructor accepts a real or implicit stage.
#[derive(Debug)]
pub struct LocalDevelopmentConfig {
    tls: PostgresTlsConfig,
}

impl LocalDevelopmentConfig {
    pub fn from_lookup(
        app: &str,
        mut get: impl FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, LocalDatabaseError> {
        let stage = get("STAGE").ok_or(PostgresPoolConfigError::MissingInput("STAGE"))?;
        if !matches!(stage.as_str(), "local" | "ephemeral" | "test") {
            return Err(LocalDatabaseError::LocalStageRequired);
        }
        // Use the same stage snapshot for both the bootstrap gate and shared TLS policy.
        let tls = PostgresTlsConfig::from_lookup(app, |key| {
            if key == "STAGE" {
                Some(stage.clone())
            } else {
                get(key)
            }
        })?;
        Ok(Self { tls })
    }

    pub fn pool_config(
        &self,
        db_name: &str,
        max_connections: u32,
    ) -> Result<PostgresPoolConfig, PostgresPoolConfigError> {
        PostgresPoolConfig::from_url(
            &database_url(self, db_name),
            max_connections,
            self.tls.clone(),
        )
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

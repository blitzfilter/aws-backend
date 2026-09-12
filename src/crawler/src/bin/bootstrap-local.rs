//! Explicit non-real-stage Docker/database/migration bootstrap. No crawler or provider work.
use crawler::local_db::{
    LocalDatabaseError, LocalDevelopmentConfig, bootstrap_all_local_databases,
    parse_postgres_environment,
};

#[tokio::main]
async fn main() -> Result<(), LocalDatabaseError> {
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err(LocalDatabaseError::Config(
            platform_postgres::PostgresPoolConfigError::InvalidInput(".env"),
        ));
    }
    let local = parse_postgres_environment(
        |key| std::env::var(key),
        |get| LocalDevelopmentConfig::from_lookup("crawler-bootstrap-local", get),
    )?;
    bootstrap_all_local_databases(&local).await?;
    println!("Local crawler databases ready; migrations applied.");
    Ok(())
}

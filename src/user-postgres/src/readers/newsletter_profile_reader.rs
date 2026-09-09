use crate::mapping::{parse_optional_currency, parse_optional_language};
use application::error::box_error;
use sqlx::FromRow;
use user_core::user_id::UserId;
use user_core::{first_name::FirstName, last_name::LastName};
use user_service::ports::{NewsletterProfile, NewsletterProfileReadError, NewsletterProfileReader};

#[derive(Debug, Clone)]
pub struct SqlxNewsletterProfileReader {
    pool: sqlx::PgPool,
}

#[derive(Debug, FromRow)]
struct NewsletterProfileRow {
    first_name: Option<String>,
    last_name: Option<String>,
    language: Option<String>,
    currency: Option<String>,
}

#[derive(Debug, thiserror::Error)]
enum NewsletterProfileRowMappingError {
    #[error("invalid newsletter profile language")]
    InvalidLanguage,
    #[error("invalid newsletter profile currency")]
    InvalidCurrency,
}

impl SqlxNewsletterProfileReader {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl NewsletterProfileReader for SqlxNewsletterProfileReader {
    async fn find_by_user_id(
        &self,
        user_id: UserId,
    ) -> Result<Option<NewsletterProfile>, NewsletterProfileReadError> {
        let row = sqlx::query_as::<_, NewsletterProfileRow>(
            "SELECT first_name, last_name, language, currency FROM users WHERE user_id = $1",
        )
        .bind(user_id.into_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(
            |source| NewsletterProfileReadError::TemporarilyUnavailable {
                source: box_error(source),
            },
        )?;

        row.map(NewsletterProfile::try_from)
            .transpose()
            .map_err(|source| NewsletterProfileReadError::InvalidReadModel {
                source: box_error(source),
            })
    }
}

impl TryFrom<NewsletterProfileRow> for NewsletterProfile {
    type Error = NewsletterProfileRowMappingError;

    fn try_from(row: NewsletterProfileRow) -> Result<Self, Self::Error> {
        Ok(Self {
            first_name: row.first_name.map(FirstName::from),
            last_name: row.last_name.map(LastName::from),
            language: parse_optional_language(row.language.as_deref())
                .map_err(|_| NewsletterProfileRowMappingError::InvalidLanguage)?,
            currency: parse_optional_currency(row.currency.as_deref())
                .map_err(|_| NewsletterProfileRowMappingError::InvalidCurrency)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

    const BUSINESS_SCHEMA: Postgres = Postgres::new_schema_once("migrations");

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_read_newsletter_profile_when_user_exists() {
        let pool = get_postgres_client().await;
        let user_id = UserId::new();
        sqlx::query(
            "INSERT INTO users (user_id, email, first_name, last_name, language, currency, tier, role) VALUES ($1, $2, $3, $4, $5, $6, 'FREE', 'USER')",
        )
        .bind(user_id.into_uuid())
        .bind(format!("{user_id}@example.test"))
        .bind("Ada")
        .bind("Lovelace")
        .bind("en")
        .bind("EUR")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed to seed newsletter profile: {error}"));

        let profile = SqlxNewsletterProfileReader::new(pool)
            .find_by_user_id(user_id)
            .await
            .unwrap_or_else(|error| panic!("failed to read newsletter profile: {error}"));

        assert_eq!(
            Some(NewsletterProfile {
                first_name: Some(FirstName::from("Ada")),
                last_name: Some(LastName::from("Lovelace")),
                language: Some(localization::Language::En),
                currency: Some(money::Currency::Eur),
            }),
            profile
        );
    }

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_return_none_when_user_does_not_exist() {
        let profile = SqlxNewsletterProfileReader::new(get_postgres_client().await)
            .find_by_user_id(UserId::new())
            .await
            .unwrap_or_else(|error| panic!("failed to read newsletter profile: {error}"));

        assert_eq!(None, profile);
    }
}

mod listing_source_details_reader;
mod listing_source_search_reader;
mod public_listing_source;
mod public_listing_source_details_reader;
mod public_listing_source_search_reader;
mod shopify_source_reader;
mod web_crawl_source_reader;
mod woocommerce_signature_verifier;
mod woocommerce_source_reader;

pub use listing_source_search_reader::SqlxListingSourceSearchReaderFactory;
pub use public_listing_source_details_reader::SqlxPublicListingSourceDetailsReaderFactory;
pub use public_listing_source_search_reader::SqlxPublicListingSourceSearchReaderFactory;

use application::error::box_error;
use listing_source_service::ports::ListingSourceReadError;
use sqlx::PgPool;

#[derive(Clone)]
pub struct SqlxListingSourceReaders {
    pub(super) pool: PgPool,
}

impl SqlxListingSourceReaders {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

pub(super) fn read_error(error: sqlx::Error) -> ListingSourceReadError {
    ListingSourceReadError::TemporarilyUnavailable {
        source: box_error(error),
    }
}

pub(super) fn invalid_read(
    error: impl std::error::Error + Send + Sync + 'static,
) -> ListingSourceReadError {
    ListingSourceReadError::InvalidReadModel {
        source: box_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::SqlxListingSourceReaders;
    use listing_source_core::{Domain, ListingSourceId};
    use listing_source_service::ports::{
        ShopifySourceReader, WoocommerceSignatureVerification, WoocommerceSignatureVerifier,
        WoocommerceSourceReader,
    };
    use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};

    const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

    #[aura_integration_test(services = [BUSINESS_SCHEMA])]
    async fn should_require_the_operator_partnership_exact_source_grant_for_provider_reads() {
        let pool = get_postgres_client().await;
        let source_id = ListingSourceId::new();
        let operator_party_id = uuid::Uuid::now_v7();
        let unrelated_party_id = uuid::Uuid::now_v7();
        let operator_partnership_id = uuid::Uuid::now_v7();
        let unrelated_partnership_id = uuid::Uuid::now_v7();
        let domain = Domain::try_from("shop.provider-reader.example")
            .unwrap_or_else(|error| panic!("valid test domain: {error}"));

        for (party_id, slug, name) in [
            (
                operator_party_id,
                "provider-reader-operator",
                "Provider reader operator",
            ),
            (
                unrelated_party_id,
                "provider-reader-unrelated",
                "Provider reader unrelated",
            ),
        ] {
            sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
                .bind(party_id)
                .bind(slug)
                .bind(name)
                .execute(&pool)
                .await
                .unwrap_or_else(|error| panic!("insert test party: {error}"));
        }
        sqlx::query(
            "INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(source_id.into_uuid())
        .bind(format!(
                    "provider-reader-source-{}",
                    source_id.as_uuid().simple()
                ))
        .bind("Provider reader source")
        .bind(operator_party_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert provider listing source: {error}"));
        sqlx::query(
            "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'SHOPIFY'), ($1, 'WOOCOMMERCE')",
        )
        .bind(source_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert provider ingestion methods: {error}"));
        sqlx::query(
            "INSERT INTO listing_source_shopify_ingestion_configurations (listing_source_id, domain) VALUES ($1, $2)",
        )
        .bind(source_id.into_uuid())
        .bind(domain.as_str())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert Shopify configuration: {error}"));
        sqlx::query(
            "INSERT INTO listing_source_woocommerce_ingestion_configurations (listing_source_id, webhook_secret) VALUES ($1, $2)",
        )
        .bind(source_id.into_uuid())
        .bind("provider-reader-secret")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert WooCommerce configuration: {error}"));
        for (partnership_id, party_id) in [
            (operator_partnership_id, operator_party_id),
            (unrelated_partnership_id, unrelated_party_id),
        ] {
            sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
                .bind(partnership_id)
                .bind(party_id)
                .execute(&pool)
                .await
                .unwrap_or_else(|error| panic!("insert test partnership: {error}"));
        }
        sqlx::query(
            "INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)",
        )
        .bind(unrelated_partnership_id)
        .bind(source_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert unrelated source grant: {error}"));

        let readers = SqlxListingSourceReaders::new(pool.clone());
        assert!(
            readers
                .find_by_domain(&domain)
                .await
                .unwrap_or_else(|error| panic!("read ungranted Shopify source: {error}"))
                .is_none()
        );
        assert!(
            readers
                .find_by_id(source_id)
                .await
                .unwrap_or_else(|error| panic!("read ungranted WooCommerce source: {error}"))
                .is_none()
        );
        let body = b"payload";
        let signature = hmac_sha256(b"provider-reader-secret", body)
            .unwrap_or_else(|error| panic!("sign webhook: {error}"));
        assert_eq!(
            WoocommerceSignatureVerification::SecretNotConfigured,
            readers
                .verify(source_id, body, &signature)
                .await
                .unwrap_or_else(|error| panic!("verify ungranted WooCommerce signature: {error}"))
        );

        sqlx::query(
            "INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)",
        )
        .bind(operator_partnership_id)
        .bind(source_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("insert exact source grant: {error}"));

        assert!(matches!(
            readers
                .find_by_domain(&domain)
                .await
                .unwrap_or_else(|error| panic!("read granted Shopify source: {error}")),
            Some(source) if source.listing_source_id == source_id
        ));
        assert!(matches!(
            readers
                .find_by_id(source_id)
                .await
                .unwrap_or_else(|error| panic!("read granted WooCommerce source: {error}")),
            Some(source) if source.listing_source_id == source_id
        ));
        assert_eq!(
            WoocommerceSignatureVerification::Valid,
            readers
                .verify(source_id, body, &signature)
                .await
                .unwrap_or_else(|error| panic!("verify granted WooCommerce signature: {error}"))
        );
    }

    fn hmac_sha256(secret: &[u8], body: &[u8]) -> Result<Vec<u8>, openssl::error::ErrorStack> {
        use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};

        let key = PKey::hmac(secret)?;
        let mut signer = Signer::new(MessageDigest::sha256(), &key)?;
        signer.update(body)?;
        signer.sign_to_vec()
    }
}

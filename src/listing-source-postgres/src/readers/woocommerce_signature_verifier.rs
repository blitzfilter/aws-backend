use super::{SqlxListingSourceReaders, read_error};
use application::error::box_error;
use listing_source_core::ListingSourceId;
use listing_source_service::ports::{
    ListingSourceReadError, WoocommerceSignatureVerification, WoocommerceSignatureVerifier,
};

#[async_trait::async_trait]
impl WoocommerceSignatureVerifier for SqlxListingSourceReaders {
    async fn verify(
        &self,
        id: ListingSourceId,
        body: &[u8],
        signature: &[u8],
    ) -> Result<WoocommerceSignatureVerification, ListingSourceReadError> {
        let secret = sqlx::query_scalar::<_, Option<String>>(
            "SELECT c.webhook_secret \
             FROM listing_source_woocommerce_ingestion_configurations c \
             JOIN listing_sources s ON s.listing_source_id=c.listing_source_id \
             JOIN listing_source_ingestion_methods m \
               ON m.listing_source_id=c.listing_source_id AND m.ingestion_method='WOOCOMMERCE' \
             JOIN partnerships p ON p.party_id=s.operator_party_id \
             WHERE c.listing_source_id=$1 \
               AND EXISTS ( \
                   SELECT 1 \
                   FROM partnership_listing_source_grants source_grant \
                   WHERE source_grant.partnership_id=p.partnership_id \
                     AND source_grant.listing_source_id=c.listing_source_id \
               )",
        )
        .bind(id.into_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(read_error)?
        .flatten();
        let Some(secret) = secret else {
            return Ok(WoocommerceSignatureVerification::SecretNotConfigured);
        };
        use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
        let key = PKey::hmac(secret.as_bytes()).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?;
        let mut signer = Signer::new(MessageDigest::sha256(), &key).map_err(|error| {
            ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            }
        })?;
        signer
            .update(body)
            .map_err(|error| ListingSourceReadError::InvalidReadModel {
                source: box_error(error),
            })?;
        let expected =
            signer
                .sign_to_vec()
                .map_err(|error| ListingSourceReadError::InvalidReadModel {
                    source: box_error(error),
                })?;
        Ok(
            if expected.len() == signature.len() && openssl::memcmp::eq(&expected, signature) {
                WoocommerceSignatureVerification::Valid
            } else {
                WoocommerceSignatureVerification::Invalid
            },
        )
    }
}

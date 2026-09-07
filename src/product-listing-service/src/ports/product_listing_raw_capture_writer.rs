use application::error::BoxError;
use async_trait::async_trait;
use listing_source_core::ListingSourceId;
use product_listing_normalization::{
    NormalizationInputHash, ProductListingNormalizationInput, RawProductListingProvenance,
};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use time::OffsetDateTime;
use uuid::Uuid;

const SHA256_BYTES: usize = 32;
pub const MAX_PROVIDER_RECEIPT_SCOPE_UTF8_BYTES: usize = 128;
pub const MAX_PROVIDER_RECEIPT_DELIVERY_ID_UTF8_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductListingRawStreamId(Uuid);

impl ProductListingRawStreamId {
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl From<ProductListingRawStreamId> for Uuid {
    fn from(value: ProductListingRawStreamId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProductListingRawRevisionId(Uuid);

impl ProductListingRawRevisionId {
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl From<ProductListingRawRevisionId> for Uuid {
    fn from(value: ProductListingRawRevisionId) -> Self {
        value.0
    }
}

/// Raw ingestion methods intentionally exclude `PARTNER_API`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum ProductListingRawIngestionMethod {
    WebCrawl,
    Shopify,
    Woocommerce,
}

impl ProductListingRawIngestionMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebCrawl => "WEB_CRAWL",
            Self::Shopify => "SHOPIFY",
            Self::Woocommerce => "WOOCOMMERCE",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        Self::iter().find(|method| method.as_str() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceRecordKeySha256([u8; SHA256_BYTES]);

impl SourceRecordKeySha256 {
    pub const fn new(value: [u8; SHA256_BYTES]) -> Self {
        Self(value)
    }

    pub const fn as_bytes(&self) -> &[u8; SHA256_BYTES] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderReceiptScope(String);

#[derive(Debug, thiserror::Error)]
pub enum ProviderReceiptScopeError {
    #[error("provider receipt scope is required")]
    Empty,
    #[error("provider receipt scope exceeds the maximum UTF-8 byte length")]
    TooLong { len: usize, max: usize },
    #[error("provider receipt scope contains an embedded NUL")]
    EmbeddedNul,
}

impl ProviderReceiptScope {
    pub fn new(value: String) -> Result<Self, ProviderReceiptScopeError> {
        if value.is_empty() {
            return Err(ProviderReceiptScopeError::Empty);
        }
        if value.len() > MAX_PROVIDER_RECEIPT_SCOPE_UTF8_BYTES {
            return Err(ProviderReceiptScopeError::TooLong {
                len: value.len(),
                max: MAX_PROVIDER_RECEIPT_SCOPE_UTF8_BYTES,
            });
        }
        if value.contains('\0') {
            return Err(ProviderReceiptScopeError::EmbeddedNul);
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceEvidenceSha256([u8; SHA256_BYTES]);

impl SourceEvidenceSha256 {
    pub const fn new(value: [u8; SHA256_BYTES]) -> Self {
        Self(value)
    }

    pub const fn as_bytes(&self) -> &[u8; SHA256_BYTES] {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderReceiptDeliveryIdError {
    #[error("provider receipt delivery ID is required")]
    Empty,
    #[error("provider receipt delivery ID exceeds the maximum UTF-8 byte length")]
    TooLong { len: usize, max: usize },
    #[error("provider receipt delivery ID contains an embedded NUL")]
    EmbeddedNul,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductListingRawProviderReceipt {
    scope: ProviderReceiptScope,
    delivery_id: String,
    source_evidence_sha256: SourceEvidenceSha256,
}

impl ProductListingRawProviderReceipt {
    pub fn new(
        scope: ProviderReceiptScope,
        delivery_id: String,
        source_evidence_sha256: SourceEvidenceSha256,
    ) -> Result<Self, ProviderReceiptDeliveryIdError> {
        if delivery_id.is_empty() {
            return Err(ProviderReceiptDeliveryIdError::Empty);
        }
        if delivery_id.len() > MAX_PROVIDER_RECEIPT_DELIVERY_ID_UTF8_BYTES {
            return Err(ProviderReceiptDeliveryIdError::TooLong {
                len: delivery_id.len(),
                max: MAX_PROVIDER_RECEIPT_DELIVERY_ID_UTF8_BYTES,
            });
        }
        if delivery_id.contains('\0') {
            return Err(ProviderReceiptDeliveryIdError::EmbeddedNul);
        }

        Ok(Self {
            scope,
            delivery_id,
            source_evidence_sha256,
        })
    }

    pub fn scope(&self) -> &ProviderReceiptScope {
        &self.scope
    }

    pub fn delivery_id(&self) -> &str {
        &self.delivery_id
    }

    pub const fn source_evidence_sha256(&self) -> &SourceEvidenceSha256 {
        &self.source_evidence_sha256
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductListingRawCaptureWrite {
    pub listing_source_id: ListingSourceId,
    pub ingestion_method: ProductListingRawIngestionMethod,
    pub source_record_key: String,
    pub source_record_key_sha256: SourceRecordKeySha256,
    pub input: ProductListingNormalizationInput,
    pub input_sha256: NormalizationInputHash,
    pub provenance: RawProductListingProvenance,
    pub source_event_id: Option<String>,
    pub source_occurred_at: Option<OffsetDateTime>,
    pub provider_receipt: Option<ProductListingRawProviderReceipt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductListingRawCaptureWriteOutcome {
    Changed {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        product_listing_raw_revision_id: ProductListingRawRevisionId,
        revision: u64,
    },
    Unchanged {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        latest_revision: u64,
    },
    Duplicate {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        latest_revision: u64,
    },
    Stale {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        latest_revision: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProductListingRawCaptureWriteError {
    #[error("source record key hash collision")]
    SourceRecordKeyHashCollision,
    #[error("provider delivery ID conflicts with existing source evidence")]
    ProviderReceiptDigestConflict,
    #[error("provider source order conflicts with existing source evidence")]
    ProviderSourceOrderConflict,
    #[error("raw product listing capture failed")]
    CaptureFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait]
pub trait ProductListingRawCaptureWriter: Send {
    async fn capture(
        &mut self,
        write: ProductListingRawCaptureWrite,
    ) -> Result<ProductListingRawCaptureWriteOutcome, ProductListingRawCaptureWriteError>;
}

pub trait ProductListingRawCaptureWriterFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx)
    -> impl ProductListingRawCaptureWriter + 'tx;
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_PROVIDER_RECEIPT_DELIVERY_ID_UTF8_BYTES, MAX_PROVIDER_RECEIPT_SCOPE_UTF8_BYTES,
        ProductListingRawProviderReceipt, ProviderReceiptDeliveryIdError, ProviderReceiptScope,
        ProviderReceiptScopeError, SHA256_BYTES, SourceEvidenceSha256,
    };

    #[test]
    fn should_accept_provider_receipt_scope_at_maximum_length()
    -> Result<(), ProviderReceiptScopeError> {
        let value = "a".repeat(MAX_PROVIDER_RECEIPT_SCOPE_UTF8_BYTES);
        let scope = ProviderReceiptScope::new(value)?;

        assert_eq!(scope.as_str().len(), MAX_PROVIDER_RECEIPT_SCOPE_UTF8_BYTES);

        Ok(())
    }

    #[test]
    fn should_reject_invalid_provider_receipt_scope() {
        assert!(matches!(
            ProviderReceiptScope::new(String::new()),
            Err(ProviderReceiptScopeError::Empty)
        ));
        assert!(matches!(
            ProviderReceiptScope::new("a".repeat(MAX_PROVIDER_RECEIPT_SCOPE_UTF8_BYTES + 1)),
            Err(ProviderReceiptScopeError::TooLong { .. })
        ));
        assert!(matches!(
            ProviderReceiptScope::new("scope\0suffix".to_owned()),
            Err(ProviderReceiptScopeError::EmbeddedNul)
        ));
    }

    #[test]
    fn should_accept_maximum_length_provider_receipt_delivery_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let receipt = ProductListingRawProviderReceipt::new(
            provider_receipt_scope()?,
            "a".repeat(MAX_PROVIDER_RECEIPT_DELIVERY_ID_UTF8_BYTES),
            SourceEvidenceSha256::new([7; SHA256_BYTES]),
        )?;

        assert_eq!(receipt.scope().as_str(), "provider:product");
        assert_eq!(
            receipt.delivery_id().len(),
            MAX_PROVIDER_RECEIPT_DELIVERY_ID_UTF8_BYTES
        );
        assert_eq!(
            receipt.source_evidence_sha256().as_bytes(),
            &[7; SHA256_BYTES]
        );

        Ok(())
    }

    #[test]
    fn should_reject_invalid_provider_receipt_delivery_id() -> Result<(), ProviderReceiptScopeError>
    {
        assert!(matches!(
            ProductListingRawProviderReceipt::new(
                provider_receipt_scope()?,
                String::new(),
                SourceEvidenceSha256::new([0; SHA256_BYTES]),
            ),
            Err(ProviderReceiptDeliveryIdError::Empty)
        ));
        assert!(matches!(
            ProductListingRawProviderReceipt::new(
                provider_receipt_scope()?,
                "a".repeat(MAX_PROVIDER_RECEIPT_DELIVERY_ID_UTF8_BYTES + 1),
                SourceEvidenceSha256::new([0; SHA256_BYTES]),
            ),
            Err(ProviderReceiptDeliveryIdError::TooLong { .. })
        ));
        assert!(matches!(
            ProductListingRawProviderReceipt::new(
                provider_receipt_scope()?,
                "delivery\0suffix".to_owned(),
                SourceEvidenceSha256::new([0; SHA256_BYTES]),
            ),
            Err(ProviderReceiptDeliveryIdError::EmbeddedNul)
        ));

        Ok(())
    }

    fn provider_receipt_scope() -> Result<ProviderReceiptScope, ProviderReceiptScopeError> {
        ProviderReceiptScope::new("provider:product".to_owned())
    }
}

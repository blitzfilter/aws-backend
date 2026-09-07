mod types;

pub use types::{
    ShopifyEventDetail, ShopifyEventMetadata, ShopifyImagePayload, ShopifyListingAction,
    ShopifyProductEventError, ShopifyProductEventKind, ShopifyProductPayload,
    ShopifyRawObservation, ShopifyVariantPayload, fallbacked_html_to_markdown,
    product_availability,
};

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{BatchItemFailure, SqsBatchResponse, SqsEvent};
use lambda_runtime::LambdaEvent;
use listing_source_core::Domain;
use listing_source_service::ports::{ListingSourceReadError, ShopifySourceReader};
use product_listing_normalization::{RawProductListingProvenance, SourcePayload};
use product_listing_service::ports::{
    ProductListingRawIngestionMethod, ProductListingRawProviderReceipt,
    ProviderReceiptDeliveryIdError, ProviderReceiptScope, ProviderReceiptScopeError,
    SourceEvidenceSha256,
};
use product_listing_service::use_cases::{
    CaptureProductListingRawObservationCommand, CaptureProductListingRawObservationError,
    CaptureProductListingRawObservationUseCase,
};
use serde_json::{Value, json};
use tracing::{info, warn};

pub const SHOPIFY_TOPIC_PRODUCTS_CREATE: &str = "products/create";
pub const SHOPIFY_TOPIC_PRODUCTS_UPDATE: &str = "products/update";
pub const SHOPIFY_TOPIC_PRODUCTS_DELETE: &str = "products/delete";

const SHOPIFY_WEBHOOK_RECEIPT_DELIVERY_ID_PREFIX: &str = "shopify-webhook:";
const EVENTBRIDGE_RECEIPT_DELIVERY_ID_PREFIX: &str = "eventbridge:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageOutcome {
    Acknowledged,
    Retry,
}

#[derive(Debug, Clone)]
pub struct ShopifyEventProvenance {
    pub topic: String,
    pub shopify_event_id: Option<String>,
    pub webhook_id: Option<String>,
    pub event_bridge_event_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProviderReceiptError {
    #[error("Shopify provider receipt scope is invalid")]
    Scope(#[source] ProviderReceiptScopeError),
    #[error("Shopify provider receipt delivery ID is invalid")]
    DeliveryId(#[source] ProviderReceiptDeliveryIdError),
    #[error("Shopify provider receipt source payload is invalid")]
    SourcePayload(#[source] product_listing_normalization::NormalizationInputError),
}

#[derive(Debug, thiserror::Error)]
pub enum ShopifyProductListingProcessingError {
    #[error("Shopify product payload is invalid")]
    InvalidPayload(#[source] ShopifyProductEventError),
    #[error("Shopify raw product provenance is invalid")]
    InvalidProvenance(#[source] product_listing_normalization::NormalizationInputError),
    #[error("Shopify provider receipt is invalid")]
    InvalidProviderReceipt(#[source] ShopifyProviderReceiptError),
    #[error("Listing source lookup failed")]
    ListingSourceLookup(#[source] ListingSourceReadError),
    #[error("Shopify raw product capture failed")]
    Capture(#[source] CaptureProductListingRawObservationError),
}

#[async_trait::async_trait]
pub trait ShopifyProductListingProcessorUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        shop_domain: Domain,
        payload: Value,
        provenance: ShopifyEventProvenance,
    ) -> Result<(), ShopifyProductListingProcessingError>;
}

pub struct ShopifyProductListingProcessor<S, C> {
    sources: S,
    capture: C,
}

impl<S, C> ShopifyProductListingProcessor<S, C> {
    pub fn new(sources: S, capture: C) -> Self {
        Self { sources, capture }
    }
}

#[async_trait::async_trait]
impl<S, C> ShopifyProductListingProcessorUseCase for ShopifyProductListingProcessor<S, C>
where
    S: ShopifySourceReader,
    C: CaptureProductListingRawObservationUseCase,
{
    async fn execute(
        &self,
        context: &OperationContext,
        kind: ShopifyProductEventKind,
        source_domain: Domain,
        payload: Value,
        provenance: ShopifyEventProvenance,
    ) -> Result<(), ShopifyProductListingProcessingError> {
        let Some(source) = self
            .sources
            .find_by_domain(&source_domain)
            .await
            .map_err(ShopifyProductListingProcessingError::ListingSourceLookup)?
        else {
            return Ok(());
        };
        let ShopifyListingAction::Capture(observation) = kind
            .listing_action(&source, payload)
            .map_err(ShopifyProductListingProcessingError::InvalidPayload)?
        else {
            return Ok(());
        };
        let provider_receipt = shopify_provider_receipt(
            provenance.topic.as_str(),
            provenance.webhook_id.as_deref(),
            provenance.event_bridge_event_id.as_deref(),
            observation.input.source_payload(),
        )
        .map_err(ShopifyProductListingProcessingError::InvalidProviderReceipt)?;
        let raw_provenance = RawProductListingProvenance::new(json!({
            "topic": &provenance.topic,
            "shopifyEventId": &provenance.shopify_event_id,
            "shopifyWebhookId": &provenance.webhook_id,
            "eventBridgeEventId": &provenance.event_bridge_event_id,
        }))
        .map_err(ShopifyProductListingProcessingError::InvalidProvenance)?;

        self.capture
            .execute(
                context,
                CaptureProductListingRawObservationCommand {
                    listing_source_id: source.listing_source_id,
                    ingestion_method: ProductListingRawIngestionMethod::Shopify,
                    source_record_key: observation.source_record_key,
                    input: observation.input,
                    provenance: raw_provenance,
                    source_event_id: provenance.shopify_event_id,
                    source_occurred_at: observation.source_occurred_at,
                    provider_receipt,
                },
            )
            .await
            .map(|_| ())
            .map_err(ShopifyProductListingProcessingError::Capture)
    }
}

#[tracing::instrument(
    skip(event, processor),
    fields(
        event_bridge_event_id = tracing::field::Empty,
        shopify_event_id = tracing::field::Empty,
        shopify_webhook_id = tracing::field::Empty,
        shopify_topic = tracing::field::Empty,
        shopify_domain = tracing::field::Empty,
    )
)]
async fn process_event(
    event: EventBridgeEvent<Value>,
    context: &OperationContext,
    processor: &(dyn ShopifyProductListingProcessorUseCase + Send + Sync),
) -> MessageOutcome {
    let span = tracing::Span::current();
    if let Some(event_id) = event.id.as_deref() {
        span.record("event_bridge_event_id", event_id);
    }
    let event_bridge_event_id = event.id;
    let detail = match serde_json::from_value::<ShopifyEventDetail>(event.detail) {
        Ok(detail) => detail,
        Err(error) => {
            warn!(%error, "Shopify event detail is malformed; retrying SQS message");
            return MessageOutcome::Retry;
        }
    };
    if let Some(event_id) = detail.metadata.event_id.as_deref() {
        span.record("shopify_event_id", event_id);
    }
    if let Some(webhook_id) = detail.metadata.webhook_id.as_deref() {
        span.record("shopify_webhook_id", webhook_id);
    }
    span.record("shopify_topic", detail.metadata.topic.as_str());
    span.record("shopify_domain", detail.metadata.shop_domain.as_str());

    let kind = match detail.metadata.topic.as_str() {
        SHOPIFY_TOPIC_PRODUCTS_CREATE => ShopifyProductEventKind::Create,
        SHOPIFY_TOPIC_PRODUCTS_UPDATE => ShopifyProductEventKind::Update,
        SHOPIFY_TOPIC_PRODUCTS_DELETE => ShopifyProductEventKind::Delete,
        _ => return MessageOutcome::Acknowledged,
    };
    let shop_domain = match Domain::try_from(detail.metadata.shop_domain.as_str()) {
        Ok(domain) => domain,
        Err(error) => {
            warn!(%error, "Shopify event has invalid shop domain; acknowledging message");
            return MessageOutcome::Acknowledged;
        }
    };
    let provenance = ShopifyEventProvenance {
        topic: detail.metadata.topic,
        shopify_event_id: detail.metadata.event_id,
        webhook_id: detail.metadata.webhook_id,
        event_bridge_event_id,
    };
    match processor
        .execute(context, kind, shop_domain, detail.payload, provenance)
        .await
    {
        Ok(()) => MessageOutcome::Acknowledged,
        Err(error) if should_retry(&error) => {
            warn!(%error, "Shopify product processing failed; retrying SQS message");
            MessageOutcome::Retry
        }
        Err(error) => {
            warn!(%error, "Shopify product payload cannot be processed; acknowledging message");
            MessageOutcome::Acknowledged
        }
    }
}

fn should_retry(error: &ShopifyProductListingProcessingError) -> bool {
    match error {
        ShopifyProductListingProcessingError::InvalidPayload(_)
        | ShopifyProductListingProcessingError::InvalidProvenance(_)
        | ShopifyProductListingProcessingError::InvalidProviderReceipt(_) => false,
        ShopifyProductListingProcessingError::ListingSourceLookup(_) => true,
        ShopifyProductListingProcessingError::Capture(error) => !matches!(
            error,
            CaptureProductListingRawObservationError::AuthenticatedActorRequired
                | CaptureProductListingRawObservationError::Forbidden
                | CaptureProductListingRawObservationError::SourceRecordKeyTooLong { .. }
                | CaptureProductListingRawObservationError::SourceRecordKeyEmbeddedNul
                | CaptureProductListingRawObservationError::InvalidInput { .. }
                | CaptureProductListingRawObservationError::ListingSourceNotFound
                | CaptureProductListingRawObservationError::SourceRecordKeyHashCollision
                | CaptureProductListingRawObservationError::ProviderReceiptDigestConflict
        ),
    }
}

fn shopify_provider_receipt(
    topic: &str,
    webhook_id: Option<&str>,
    event_bridge_event_id: Option<&str>,
    source_payload: &SourcePayload,
) -> Result<Option<ProductListingRawProviderReceipt>, ShopifyProviderReceiptError> {
    let Some(delivery_id) = shopify_receipt_delivery_identity(webhook_id, event_bridge_event_id)
        .map_err(ShopifyProviderReceiptError::DeliveryId)?
    else {
        return Ok(None);
    };
    let scope =
        ProviderReceiptScope::new(topic.to_owned()).map_err(ShopifyProviderReceiptError::Scope)?;
    let source_evidence_sha256 = source_payload
        .canonical_sha256()
        .map_err(ShopifyProviderReceiptError::SourcePayload)?;
    ProductListingRawProviderReceipt::new(
        scope,
        delivery_id,
        SourceEvidenceSha256::new(*source_evidence_sha256.as_bytes()),
    )
    .map(Some)
    .map_err(ShopifyProviderReceiptError::DeliveryId)
}

fn shopify_receipt_delivery_identity(
    webhook_id: Option<&str>,
    event_bridge_event_id: Option<&str>,
) -> Result<Option<String>, ProviderReceiptDeliveryIdError> {
    let (prefix, delivery_id) = match webhook_id {
        Some(webhook_id) => (SHOPIFY_WEBHOOK_RECEIPT_DELIVERY_ID_PREFIX, webhook_id),
        None => match event_bridge_event_id {
            Some(event_bridge_event_id) => (
                EVENTBRIDGE_RECEIPT_DELIVERY_ID_PREFIX,
                event_bridge_event_id,
            ),
            None => return Ok(None),
        },
    };
    if delivery_id.is_empty() {
        return Err(ProviderReceiptDeliveryIdError::Empty);
    }

    Ok(Some(format!("{prefix}{delivery_id}")))
}

#[tracing::instrument(skip(event, processor), fields(request_id = %event.context.request_id))]
pub async fn handler(
    event: LambdaEvent<SqsEvent>,
    processor: &(dyn ShopifyProductListingProcessorUseCase + Send + Sync),
) -> Result<SqsBatchResponse, lambda_runtime::Error> {
    let context = operation_context(&event);
    let count = event.payload.records.len();
    let mut failed_message_ids = Vec::new();

    for message in event.payload.records {
        let Some(message_id) = message.message_id else {
            warn!("Shopify SQS message has no message ID; acknowledging message");
            continue;
        };
        let Some(body) = message.body else {
            continue;
        };
        let event = match serde_json::from_str::<EventBridgeEvent<Value>>(&body) {
            Ok(event) => event,
            Err(error) => {
                warn!(message_id = %message_id, %error, "Shopify SQS body is malformed; retrying message");
                failed_message_ids.push(message_id);
                continue;
            }
        };
        if process_event(event, &context, processor).await == MessageOutcome::Retry {
            failed_message_ids.push(message_id);
        }
    }

    info!(
        sqs_message_count = count,
        failed_sqs_message_count = failed_message_ids.len(),
        "Finished Shopify raw product capture batch"
    );

    let mut response = SqsBatchResponse::default();
    response.batch_item_failures = failed_message_ids
        .into_iter()
        .map(|item_identifier| {
            let mut failure = BatchItemFailure::default();
            failure.item_identifier = item_identifier;
            failure
        })
        .collect();
    Ok(response)
}

fn operation_context(event: &LambdaEvent<SqsEvent>) -> OperationContext {
    let request_id = RequestId::new(event.context.request_id.clone());
    OperationContext {
        principal: Principal::System,
        correlation_id: CorrelationId::new(request_id.as_str()),
        request_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lambda_events::sqs::SqsMessage;
    use lambda_runtime::Context;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn should_acknowledge_valid_shopify_message() {
        let processor = FakeProcessor::success();
        let result = handler(event("msg-1", valid_body()), &processor)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert!(result.batch_item_failures.is_empty());
        assert_eq!(1, call_count(&processor));
    }

    #[tokio::test]
    async fn should_retry_when_raw_capture_fails_transiently() {
        let processor = FakeProcessor::failure();
        let result = handler(event("msg-1", valid_body()), &processor)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert_eq!(vec!["msg-1"], identifiers(result));
    }

    #[test]
    fn should_retry_distinct_shopify_delivery_with_conflicting_source_order() {
        assert!(should_retry(
            &ShopifyProductListingProcessingError::Capture(
                CaptureProductListingRawObservationError::ProviderSourceOrderConflict,
            )
        ));
        assert!(!should_retry(
            &ShopifyProductListingProcessingError::Capture(
                CaptureProductListingRawObservationError::ProviderReceiptDigestConflict,
            )
        ));
    }

    #[tokio::test]
    async fn should_only_fail_conflicted_record_in_shopify_batch() {
        let processor = FakeProcessor::source_order_conflict_on_second_call();
        let result = handler(
            events(vec![("valid", valid_body()), ("conflicted", valid_body())]),
            &processor,
        )
        .await
        .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert_eq!(vec!["conflicted"], identifiers(result));
        assert_eq!(2, call_count(&processor));
    }

    #[test]
    fn should_namespace_receipt_delivery_identity_by_origin() {
        let webhook_identity =
            shopify_receipt_delivery_identity(Some("same-delivery-id"), Some("same-delivery-id"))
                .unwrap_or_else(|error| panic!("webhook identity failed: {error}"));
        let eventbridge_identity =
            shopify_receipt_delivery_identity(None, Some("same-delivery-id"))
                .unwrap_or_else(|error| panic!("EventBridge identity failed: {error}"));

        assert_eq!(
            Some("shopify-webhook:same-delivery-id".to_owned()),
            webhook_identity
        );
        assert_eq!(
            Some("eventbridge:same-delivery-id".to_owned()),
            eventbridge_identity
        );
        assert!(matches!(
            shopify_receipt_delivery_identity(Some(""), None),
            Err(ProviderReceiptDeliveryIdError::Empty)
        ));
    }

    #[tokio::test]
    async fn should_retry_when_sqs_body_is_invalid() {
        let result = handler(
            event("msg-1", "not JSON".to_owned()),
            &FakeProcessor::success(),
        )
        .await
        .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert_eq!(vec!["msg-1"], identifiers(result));
    }

    #[tokio::test]
    async fn should_acknowledge_malformed_product_payload() {
        let processor = FakeProcessor::invalid_payload();
        let result = handler(event("msg-1", valid_body()), &processor)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert!(result.batch_item_failures.is_empty());
    }

    #[tokio::test]
    async fn should_acknowledge_unsupported_topic_without_capture() {
        let processor = FakeProcessor::success();
        let result = handler(event("msg-1", body_with_topic("orders/create")), &processor)
            .await
            .unwrap_or_else(|error| panic!("handler failed: {error}"));

        assert!(result.batch_item_failures.is_empty());
        assert_eq!(0, call_count(&processor));
    }

    fn event(message_id: &str, body: String) -> LambdaEvent<SqsEvent> {
        events(vec![(message_id, body)])
    }

    fn events(records: Vec<(&str, String)>) -> LambdaEvent<SqsEvent> {
        let records = records
            .into_iter()
            .map(|(message_id, body)| {
                let mut message = SqsMessage::default();
                message.message_id = Some(message_id.to_owned());
                message.body = Some(body);
                message
            })
            .collect();
        let mut sqs_event = SqsEvent::default();
        sqs_event.records = records;
        LambdaEvent::new(sqs_event, Context::default())
    }

    fn valid_body() -> String {
        body_with_topic(SHOPIFY_TOPIC_PRODUCTS_CREATE)
    }

    fn body_with_topic(topic: &str) -> String {
        let mut event = EventBridgeEvent::<Value>::default();
        event.detail_type = "shopifyWebhook".to_owned();
        event.source = "aws.partner/shopify.com/test".to_owned();
        event.detail = serde_json::json!({
            "payload": {
                "id": 42,
                "title": "Cabinet",
                "handle": "cabinet",
                "status": "active",
                "variants": [{"price": "42.00", "inventory_quantity": 1}],
                "images": []
            },
            "metadata": {
                "X-Shopify-Topic": topic,
                "X-Shopify-Shop-Domain": "partner.example"
            }
        });
        serde_json::to_string(&event)
            .unwrap_or_else(|error| panic!("failed serializing EventBridge fixture: {error}"))
    }

    fn identifiers(response: SqsBatchResponse) -> Vec<String> {
        response
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect()
    }

    #[derive(Clone, Copy)]
    enum FakeResult {
        Success,
        Failure,
        InvalidPayload,
        SourceOrderConflictOnSecondCall,
    }

    #[derive(Clone)]
    struct FakeProcessor {
        calls: Arc<Mutex<usize>>,
        result: FakeResult,
    }

    impl FakeProcessor {
        fn success() -> Self {
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: FakeResult::Success,
            }
        }

        fn failure() -> Self {
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: FakeResult::Failure,
            }
        }

        fn invalid_payload() -> Self {
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: FakeResult::InvalidPayload,
            }
        }

        fn source_order_conflict_on_second_call() -> Self {
            Self {
                calls: Arc::new(Mutex::new(0)),
                result: FakeResult::SourceOrderConflictOnSecondCall,
            }
        }
    }

    fn call_count(processor: &FakeProcessor) -> usize {
        *processor
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    #[async_trait::async_trait]
    impl ShopifyProductListingProcessorUseCase for FakeProcessor {
        async fn execute(
            &self,
            _context: &OperationContext,
            _kind: ShopifyProductEventKind,
            _shop_domain: Domain,
            _payload: Value,
            _provenance: ShopifyEventProvenance,
        ) -> Result<(), ShopifyProductListingProcessingError> {
            let call_count = {
                let mut calls = self.calls.lock().unwrap_or_else(|error| error.into_inner());
                *calls += 1;
                *calls
            };
            match self.result {
                FakeResult::Success => Ok(()),
                FakeResult::Failure => Err(ShopifyProductListingProcessingError::Capture(
                    CaptureProductListingRawObservationError::CaptureFailed {
                        source: application::error::box_error(std::io::Error::other("temporary")),
                    },
                )),
                FakeResult::InvalidPayload => {
                    Err(ShopifyProductListingProcessingError::InvalidPayload(
                        ShopifyProductEventError::MissingTitle,
                    ))
                }
                FakeResult::SourceOrderConflictOnSecondCall if call_count == 2 => {
                    Err(ShopifyProductListingProcessingError::Capture(
                        CaptureProductListingRawObservationError::ProviderSourceOrderConflict,
                    ))
                }
                FakeResult::SourceOrderConflictOnSecondCall => Ok(()),
            }
        }
    }
}

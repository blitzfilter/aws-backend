use aws_lambda_events::eventbridge::EventBridgeEvent;
use aws_lambda_events::sqs::{SqsEvent, SqsMessage};
use lambda_runtime::{Context, LambdaEvent};
use listing_source_core::{Domain, ListingSourceId};
use listing_source_postgres::SqlxListingSourceReaders;
use platform_postgres::SqlxUnitOfWork;
use product_listing_normalization::SourcePayload;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxPendingProductListingRawStreamReader,
    SqlxProductListingEventAppenderFactory, SqlxProductListingRawCaptureWriterFactory,
    SqlxProductListingRawNormalizationWriterFactory, SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::CaptureProductListingRawObservationHandler;
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionHandler,
    NormalizeProductListingRawRevisionMode, NormalizeProductListingRawRevisionUseCase,
};
use shopify_lambda::{
    SHOPIFY_TOPIC_PRODUCTS_CREATE, SHOPIFY_TOPIC_PRODUCTS_DELETE, SHOPIFY_TOPIC_PRODUCTS_UPDATE,
    ShopifyProductListingProcessor, ShopifyProductListingProcessorUseCase, handler,
};
use sqlx::types::Json;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_capture_shopify_raw_revision_without_direct_canonical_write() {
    let source = seed_source().await;

    let response = invoke(product_event(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        source.domain.as_str(),
        100,
        5,
        "shopify-event-1",
        "eventbridge-1",
        serde_json::json!({"futureShopifyKey": {"retained": true}}),
    ))
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(1, raw_revision_count(source.id, 100).await);
    assert_eq!(0, listing_count(source.id).await);
    assert_eq!(0, product_listing_event_count().await);

    let revision = raw_revision(source.id, 100, 1).await;
    assert_eq!("UPSERT", revision.operation);
    assert_eq!("SHOPIFY_PRODUCT", revision.payload_format);
    assert_eq!(2, revision.raw_values_schema_version);
    assert_eq!(
        serde_json::json!("MACHINE_DECIMAL"),
        revision.raw_values.0["priceFormat"]
    );
    assert_eq!(
        serde_json::json!(true),
        revision.source_payload.0["futureShopifyKey"]["retained"]
    );
    assert_eq!(
        serde_json::json!({"action": "SET", "value": "in stock"}),
        revision.raw_values.0["availability"]
    );
    assert_eq!(
        serde_json::json!("USD"),
        revision.normalization_context.0["fallbackCurrency"]
    );
    assert_eq!(
        serde_json::json!("de"),
        revision.normalization_context.0["fallbackLanguage"]
    );
    assert_eq!(
        serde_json::json!("shopify-event-1"),
        revision.provenance.0["shopifyEventId"]
    );
    assert_eq!(
        serde_json::json!("eventbridge-1"),
        revision.provenance.0["eventBridgeEventId"]
    );
    assert_eq!(Some("shopify-event-1".to_owned()), revision.source_event_id);
    assert_eq!(None, revision.source_occurred_at);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_collapse_equal_shopify_state_with_new_delivery_ids() {
    let source = seed_source().await;

    let first = invoke(product_event(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        source.domain.as_str(),
        101,
        5,
        "shopify-event-1",
        "eventbridge-1",
        serde_json::json!({}),
    ))
    .await;
    let second = invoke(product_event(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        source.domain.as_str(),
        101,
        5,
        "shopify-event-2",
        "eventbridge-2",
        serde_json::json!({}),
    ))
    .await;

    assert!(first.batch_item_failures.is_empty());
    assert!(second.batch_item_failures.is_empty());
    assert_eq!(1, raw_revision_count(source.id, 101).await);
    assert_eq!(0, listing_count(source.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_distinguish_shopify_webhook_and_eventbridge_delivery_ids_with_same_value() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
    let webhook_payload = shopify_payload(102, 5, serde_json::json!({"state": "webhook"}));
    let eventbridge_payload = shopify_payload(102, 0, serde_json::json!({"state": "eventbridge"}));

    let webhook = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        domain,
        webhook_payload.clone(),
        "shopify-event-webhook",
        Some("same-delivery-id"),
        "eventbridge-with-webhook",
    ))
    .await;
    let eventbridge = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        domain,
        eventbridge_payload.clone(),
        "shopify-event-eventbridge",
        None,
        "same-delivery-id",
    ))
    .await;

    assert!(webhook.batch_item_failures.is_empty());
    assert!(eventbridge.batch_item_failures.is_empty());
    assert_eq!(2, raw_revision_count(source.id, 102).await);
    assert_eq!(
        Some("shopify-event-webhook".to_owned()),
        raw_revision(source.id, 102, 1).await.source_event_id
    );
    assert_eq!(
        Some("shopify-event-eventbridge".to_owned()),
        raw_revision(source.id, 102, 2).await.source_event_id
    );
    assert_eq!(
        vec![
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCTS_UPDATE.to_owned(),
                delivery_id: "eventbridge:same-delivery-id".to_owned(),
                source_payload_sha256: source_payload_digest(&eventbridge_payload),
            },
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCTS_UPDATE.to_owned(),
                delivery_id: "shopify-webhook:same-delivery-id".to_owned(),
                source_payload_sha256: source_payload_digest(&webhook_payload),
            },
        ],
        provider_receipts(source.id, 102).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_preserve_e2_b_after_retrying_e1_a_and_capture_e3_a() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
    let e1_a = shopify_payload_with_updated_at(
        107,
        5,
        "2026-01-01T00:00:01Z",
        serde_json::json!({"state": "A"}),
    );
    let e2_b = shopify_payload_with_updated_at(
        107,
        0,
        "2026-01-01T00:00:02Z",
        serde_json::json!({"state": "B"}),
    );

    let first = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        domain,
        e1_a.clone(),
        "shopify-event-e1",
        Some("webhook-shared"),
        "eventbridge-e1",
    ))
    .await;
    let second = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        domain,
        e2_b.clone(),
        "shopify-event-e2",
        Some("webhook-shared"),
        "eventbridge-e2",
    ))
    .await;

    assert!(first.batch_item_failures.is_empty());
    assert!(second.batch_item_failures.is_empty());
    assert_eq!(2, raw_revision_count(source.id, 107).await);
    assert_eq!(
        vec![
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCTS_CREATE.to_owned(),
                delivery_id: "shopify-webhook:webhook-shared".to_owned(),
                source_payload_sha256: source_payload_digest(&e1_a),
            },
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCTS_UPDATE.to_owned(),
                delivery_id: "shopify-webhook:webhook-shared".to_owned(),
                source_payload_sha256: source_payload_digest(&e2_b),
            },
        ],
        provider_receipts(source.id, 107).await
    );

    let first_revision = raw_revision(source.id, 107, 1).await;
    assert_eq!(
        Some("shopify-event-e1".to_owned()),
        first_revision.source_event_id
    );
    assert_eq!(
        Some(occurred_at("2026-01-01T00:00:01Z")),
        first_revision.source_occurred_at
    );
    assert_eq!(
        serde_json::json!("webhook-shared"),
        first_revision.provenance.0["shopifyWebhookId"]
    );
    let second_revision = raw_revision(source.id, 107, 2).await;
    assert_eq!(
        Some("shopify-event-e2".to_owned()),
        second_revision.source_event_id
    );
    assert_eq!(
        Some(occurred_at("2026-01-01T00:00:02Z")),
        second_revision.source_occurred_at
    );

    assert_eq!(
        2,
        normalize_pending_shopify_revisions(get_postgres_client().await).await
    );
    let listing = listing_facts(source.id, 107).await;
    assert_eq!(Some("OUT_OF_STOCK".to_owned()), listing.0);

    let retry = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        domain,
        e1_a.clone(),
        "shopify-event-e1-redelivery",
        Some("webhook-shared"),
        "eventbridge-e1-redelivery",
    ))
    .await;

    assert!(retry.batch_item_failures.is_empty());
    assert_eq!(2, raw_revision_count(source.id, 107).await);
    assert_eq!(2, provider_receipts(source.id, 107).await.len());
    assert_eq!(
        0,
        normalize_pending_shopify_revisions(get_postgres_client().await).await
    );
    let listing = listing_facts(source.id, 107).await;
    assert_eq!(Some("OUT_OF_STOCK".to_owned()), listing.0);

    let e3_a = shopify_payload_with_updated_at(
        107,
        5,
        "2026-01-01T00:00:03Z",
        serde_json::json!({"state": "A"}),
    );
    let third = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        domain,
        e3_a.clone(),
        "shopify-event-e3",
        None,
        "eventbridge-e3",
    ))
    .await;

    assert!(third.batch_item_failures.is_empty());
    assert_eq!(3, raw_revision_count(source.id, 107).await);
    assert_eq!(
        Some("shopify-event-e3".to_owned()),
        raw_revision(source.id, 107, 3).await.source_event_id
    );
    assert!(
        provider_receipts(source.id, 107)
            .await
            .iter()
            .any(|receipt| {
                receipt.scope == SHOPIFY_TOPIC_PRODUCTS_UPDATE
                    && receipt.delivery_id == "eventbridge:eventbridge-e3"
                    && receipt.source_payload_sha256 == source_payload_digest(&e3_a)
            })
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_conflicting_shopify_provider_receipt_without_retry() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
    let e1_a = shopify_payload_with_updated_at(
        108,
        5,
        "2026-01-01T00:00:01Z",
        serde_json::json!({"state": "A"}),
    );
    let conflicting_b = shopify_payload_with_updated_at(
        108,
        0,
        "2026-01-01T00:00:02Z",
        serde_json::json!({"state": "B"}),
    );

    let first = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        domain,
        e1_a.clone(),
        "shopify-event-e1",
        Some("webhook-e1"),
        "eventbridge-e1",
    ))
    .await;
    let conflict = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        domain,
        conflicting_b,
        "shopify-event-e1-conflict",
        Some("webhook-e1"),
        "eventbridge-e1-conflict",
    ))
    .await;

    assert!(first.batch_item_failures.is_empty());
    assert!(conflict.batch_item_failures.is_empty());
    assert_eq!(1, raw_revision_count(source.id, 108).await);
    assert_eq!(
        vec![ProviderReceiptRow {
            scope: SHOPIFY_TOPIC_PRODUCTS_CREATE.to_owned(),
            delivery_id: "shopify-webhook:webhook-e1".to_owned(),
            source_payload_sha256: source_payload_digest(&e1_a),
        }],
        provider_receipts(source.id, 108).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_apply_shopify_updated_at_ordering_and_retry_conflicts() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
    let newer_a = shopify_payload_with_updated_at(
        109,
        5,
        "2026-01-01T00:00:02Z",
        serde_json::json!({"state": "A"}),
    );
    let older_b = shopify_payload_with_updated_at(
        109,
        0,
        "2026-01-01T00:00:01Z",
        serde_json::json!({"state": "B"}),
    );
    let same_time_c = shopify_payload_with_updated_at(
        109,
        2,
        "2026-01-01T00:00:02Z",
        serde_json::json!({"state": "C"}),
    );

    let newer = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        domain,
        newer_a.clone(),
        "shopify-event-newer",
        Some("webhook-newer"),
        "eventbridge-newer",
    ))
    .await;
    let stale = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        domain,
        older_b.clone(),
        "shopify-event-older",
        Some("webhook-older"),
        "eventbridge-older",
    ))
    .await;
    let same_time = invoke(event_with_provider_metadata(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        domain,
        same_time_c,
        "shopify-event-same-time",
        Some("webhook-same-time"),
        "eventbridge-same-time",
    ))
    .await;

    assert!(newer.batch_item_failures.is_empty());
    assert!(stale.batch_item_failures.is_empty());
    assert_eq!(
        vec!["message-eventbridge-same-time"],
        same_time
            .batch_item_failures
            .into_iter()
            .map(|failure| failure.item_identifier)
            .collect::<Vec<_>>(),
    );
    assert_eq!(1, raw_revision_count(source.id, 109).await);
    assert_eq!(
        Some(occurred_at("2026-01-01T00:00:02Z")),
        raw_revision(source.id, 109, 1).await.source_occurred_at
    );
    assert_eq!(
        vec![
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCTS_CREATE.to_owned(),
                delivery_id: "shopify-webhook:webhook-newer".to_owned(),
                source_payload_sha256: source_payload_digest(&newer_a),
            },
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCTS_UPDATE.to_owned(),
                delivery_id: "shopify-webhook:webhook-older".to_owned(),
                source_payload_sha256: source_payload_digest(&older_b),
            },
        ],
        provider_receipts(source.id, 109).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_capture_changed_inventory_and_unknown_shopify_key() {
    let source = seed_source().await;
    let domain = source.domain.as_str();

    assert!(
        invoke(product_event(
            SHOPIFY_TOPIC_PRODUCTS_CREATE,
            domain,
            102,
            5,
            "shopify-event-1",
            "eventbridge-1",
            serde_json::json!({"futureShopifyKey": "first"}),
        ))
        .await
        .batch_item_failures
        .is_empty()
    );
    assert!(
        invoke(product_event(
            SHOPIFY_TOPIC_PRODUCTS_UPDATE,
            domain,
            102,
            0,
            "shopify-event-2",
            "eventbridge-2",
            serde_json::json!({"futureShopifyKey": "second"}),
        ))
        .await
        .batch_item_failures
        .is_empty()
    );

    assert_eq!(2, raw_revision_count(source.id, 102).await);
    let revision = raw_revision(source.id, 102, 2).await;
    assert_eq!(
        serde_json::json!("second"),
        revision.source_payload.0["futureShopifyKey"]
    );
    assert_eq!(
        serde_json::json!({"action": "SET", "value": "out of stock"}),
        revision.raw_values.0["availability"]
    );
    assert_eq!(0, listing_count(source.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_normalize_shopify_machine_decimal_prices_and_preserve_provider_strings() {
    let source = seed_source().await;
    let domain = source.domain.as_str();
    let cases = [
        (111, "42.000", Some(4_200_i64)),
        (112, "42.5", Some(4_250_i64)),
        (113, "42.50", Some(4_250_i64)),
        (114, "", None),
    ];

    for &(product_id, price, _) in &cases {
        let mut payload = shopify_payload(product_id, 5, serde_json::json!({}));
        payload["variants"][0]["price"] = serde_json::json!(price);
        let event_id = format!("shopify-machine-{product_id}");
        let eventbridge_id = format!("eventbridge-machine-{product_id}");

        let response = invoke(event_with_payload(
            SHOPIFY_TOPIC_PRODUCTS_CREATE,
            domain,
            payload,
            &event_id,
            &eventbridge_id,
        ))
        .await;

        assert!(response.batch_item_failures.is_empty());
        let revision = raw_revision(source.id, product_id, 1).await;
        assert_eq!(2, revision.raw_values_schema_version);
        assert_eq!(
            serde_json::json!("MACHINE_DECIMAL"),
            revision.raw_values.0["priceFormat"]
        );
        assert_eq!(
            serde_json::json!(price),
            revision.source_payload.0["variants"][0]["price"]
        );
        let expected_price_patch = if price.is_empty() {
            serde_json::json!({"action": "CLEAR"})
        } else {
            serde_json::json!({"action": "SET", "value": price})
        };
        assert_eq!(expected_price_patch, revision.raw_values.0["price"]);
    }

    assert_eq!(
        cases.len(),
        normalize_pending_shopify_revisions(get_postgres_client().await).await
    );
    for &(product_id, _, expected_amount) in &cases {
        assert_eq!(
            expected_amount,
            listing_price_amount(source.id, product_id).await
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_preserve_current_shopify_listing_facts_after_asynchronous_normalization() {
    let source = seed_source().await;
    let response = invoke(product_event(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        source.domain.as_str(),
        103,
        5,
        "shopify-event-1",
        "eventbridge-1",
        serde_json::json!({}),
    ))
    .await;
    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, listing_count(source.id).await);

    let normalized = normalize_pending_shopify_revisions(get_postgres_client().await).await;

    assert_eq!(1, normalized);
    let listing = listing_facts(source.id, 103).await;
    assert_eq!(Some("IN_STOCK".to_owned()), listing.0);
    assert_eq!(4_200, listing.1);
    assert_eq!("USD", listing.2);
    assert_eq!(
        "https://shopify-".to_owned() + &source.id.to_string() + ".example/products/cabinet-103",
        listing.3
    );
    assert_eq!(1, product_listing_event_count().await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_capture_delete_without_direct_withdrawal() {
    let source = seed_source().await;
    let domain = source.domain.as_str();

    assert!(
        invoke(product_event(
            SHOPIFY_TOPIC_PRODUCTS_CREATE,
            domain,
            103,
            5,
            "shopify-event-1",
            "eventbridge-1",
            serde_json::json!({}),
        ))
        .await
        .batch_item_failures
        .is_empty()
    );
    assert!(
        invoke(product_event(
            SHOPIFY_TOPIC_PRODUCTS_DELETE,
            domain,
            103,
            5,
            "shopify-event-2",
            "eventbridge-2",
            serde_json::json!({}),
        ))
        .await
        .batch_item_failures
        .is_empty()
    );

    assert_eq!(2, raw_revision_count(source.id, 103).await);
    let delete_revision = raw_revision(source.id, 103, 2).await;
    assert_eq!("DELETE", delete_revision.operation);
    assert_eq!(2, delete_revision.raw_values_schema_version);
    assert_eq!(0, listing_count(source.id).await);
    assert_eq!(0, product_listing_event_count().await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_missing_source_or_ignored_status_with_invalid_updated_at_without_capture()
 {
    let missing_source = invoke(product_event(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        "missing-source.example",
        104,
        5,
        "shopify-event-1",
        "eventbridge-1",
        serde_json::json!({}),
    ))
    .await;
    assert!(missing_source.batch_item_failures.is_empty());
    assert_eq!(0, raw_revision_count_for_source_listing_id(104).await);

    let source = seed_source().await;
    let mut missing_status_payload = shopify_payload(105, 5, serde_json::json!({}));
    missing_status_payload["status"] = serde_json::Value::Null;
    missing_status_payload["updated_at"] = serde_json::json!("not-a-timestamp");
    let missing_status = invoke(event_with_payload(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        source.domain.as_str(),
        missing_status_payload,
        "shopify-event-2",
        "eventbridge-2",
    ))
    .await;
    assert!(missing_status.batch_item_failures.is_empty());
    assert_eq!(0, raw_revision_count(source.id, 105).await);

    let mut unsupported_status_payload = shopify_payload(106, 5, serde_json::json!({}));
    unsupported_status_payload["status"] = serde_json::json!("published");
    unsupported_status_payload["updated_at"] = serde_json::json!("not-a-timestamp");
    let unsupported_status = invoke(event_with_payload(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        source.domain.as_str(),
        unsupported_status_payload,
        "shopify-event-3",
        "eventbridge-3",
    ))
    .await;
    assert!(unsupported_status.batch_item_failures.is_empty());
    assert_eq!(0, raw_revision_count(source.id, 106).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_permanently_malformed_shopify_product_without_capture() {
    let source = seed_source().await;
    let mut payload = shopify_payload(106, 5, serde_json::json!({}));
    payload["id"] = serde_json::json!("not-a-decimal-id");

    let response = invoke(event_with_payload(
        SHOPIFY_TOPIC_PRODUCTS_CREATE,
        source.domain.as_str(),
        payload,
        "shopify-event-1",
        "eventbridge-1",
    ))
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, raw_revision_count_for_source_listing_id(106).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_acknowledge_invalid_shopify_updated_at_without_capture() {
    let source = seed_source().await;
    let mut payload = shopify_payload(110, 5, serde_json::json!({}));
    payload["updated_at"] = serde_json::json!("not-a-timestamp");

    let response = invoke(event_with_payload(
        SHOPIFY_TOPIC_PRODUCTS_UPDATE,
        source.domain.as_str(),
        payload,
        "shopify-event-invalid-time",
        "eventbridge-invalid-time",
    ))
    .await;

    assert!(response.batch_item_failures.is_empty());
    assert_eq!(0, raw_revision_count(source.id, 110).await);
}

async fn invoke(event: LambdaEvent<SqsEvent>) -> aws_lambda_events::sqs::SqsBatchResponse {
    let processor = shopify_product_listing_processor(get_postgres_client().await);
    match handler(event, &processor).await {
        Ok(response) => response,
        Err(error) => panic!("Shopify handler failed: {error}"),
    }
}

fn shopify_product_listing_processor(
    pool: sqlx::PgPool,
) -> impl ShopifyProductListingProcessorUseCase {
    ShopifyProductListingProcessor::new(
        SqlxListingSourceReaders::new(pool.clone()),
        CaptureProductListingRawObservationHandler::new(
            SqlxUnitOfWork::new(pool),
            SqlxProductListingRawCaptureWriterFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
        ),
    )
}

struct ShopifySourceFixture {
    id: ListingSourceId,
    domain: Domain,
}

async fn seed_source() -> ShopifySourceFixture {
    let listing_source_id = ListingSourceId::new();
    let operator_party_id = uuid::Uuid::new_v4();
    let domain = Domain::try_from(format!("shopify-{listing_source_id}.example").as_str())
        .unwrap_or_else(|error| panic!("invalid Shopify domain: {error}"));
    let pool = get_postgres_client().await;

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(operator_party_id)
        .bind(format!("operator-{operator_party_id}"))
        .bind("Shopify operator")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting source operator: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(format!("shopify-source-{listing_source_id}"))
        .bind("Shopify source")
        .bind(operator_party_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting listing source: {error}"));
    sqlx::query("INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'SHOPIFY')")
        .bind(uuid::Uuid::from(listing_source_id))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting Shopify ingestion method: {error}"));
    sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations (listing_source_id, domain, currency, language) VALUES ($1, $2, 'USD', 'de')")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain.as_str())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting Shopify source configuration: {error}"));
    let partnership_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership_id)
        .bind(operator_party_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting source operator partnership: {error}"));
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)")
        .bind(partnership_id)
        .bind(uuid::Uuid::from(listing_source_id))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed granting Shopify source access: {error}"));

    ShopifySourceFixture {
        id: listing_source_id,
        domain,
    }
}

fn product_event(
    topic: &str,
    shop_domain: &str,
    product_id: u64,
    inventory_quantity: i64,
    shopify_event_id: &str,
    event_bridge_event_id: &str,
    extra: serde_json::Value,
) -> LambdaEvent<SqsEvent> {
    event_with_payload(
        topic,
        shop_domain,
        shopify_payload(product_id, inventory_quantity, extra),
        shopify_event_id,
        event_bridge_event_id,
    )
}

fn event_with_payload(
    topic: &str,
    shop_domain: &str,
    payload: serde_json::Value,
    shopify_event_id: &str,
    event_bridge_event_id: &str,
) -> LambdaEvent<SqsEvent> {
    event_with_provider_metadata(
        topic,
        shop_domain,
        payload,
        shopify_event_id,
        None,
        event_bridge_event_id,
    )
}

fn event_with_provider_metadata(
    topic: &str,
    shop_domain: &str,
    payload: serde_json::Value,
    shopify_event_id: &str,
    shopify_webhook_id: Option<&str>,
    event_bridge_event_id: &str,
) -> LambdaEvent<SqsEvent> {
    let mut metadata = serde_json::json!({
        "X-Shopify-Topic": topic,
        "X-Shopify-Shop-Domain": shop_domain,
        "X-Shopify-Event-Id": shopify_event_id,
    });
    if let (Some(metadata), Some(shopify_webhook_id)) =
        (metadata.as_object_mut(), shopify_webhook_id)
    {
        metadata.insert(
            "X-Shopify-Webhook-Id".to_owned(),
            serde_json::Value::String(shopify_webhook_id.to_owned()),
        );
    }

    let mut event = EventBridgeEvent::default();
    event.id = Some(event_bridge_event_id.to_owned());
    event.detail_type = "shopifyWebhook".to_owned();
    event.source = "aws.partner/shopify.com/test".to_owned();
    event.detail = serde_json::json!({
        "payload": payload,
        "metadata": metadata,
    });
    let body = serde_json::to_string(&event)
        .unwrap_or_else(|error| panic!("failed serializing Shopify EventBridge fixture: {error}"));
    let mut message = SqsMessage::default();
    message.message_id = Some(format!("message-{event_bridge_event_id}"));
    message.body = Some(body);
    let mut sqs = SqsEvent::default();
    sqs.records = vec![message];
    LambdaEvent::new(sqs, Context::default())
}

fn shopify_payload(
    product_id: u64,
    inventory_quantity: i64,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "id": product_id,
        "title": "Shopify Cabinet",
        "body_html": "<p>Imported cabinet</p>",
        "handle": format!("cabinet-{product_id}"),
        "status": "active",
        "variants": [{"price": "42.00", "inventory_quantity": inventory_quantity, "inventory_management": "shopify"}],
        "images": [{"src": "https://images.example/cabinet.jpg"}]
    });
    if let (Some(target), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        target.extend(extra.clone());
    }
    payload
}

fn shopify_payload_with_updated_at(
    product_id: u64,
    inventory_quantity: i64,
    updated_at: &str,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut payload = shopify_payload(product_id, inventory_quantity, extra);
    payload["updated_at"] = serde_json::json!(updated_at);
    payload
}

#[derive(sqlx::FromRow)]
struct RawRevisionRow {
    operation: String,
    payload_format: String,
    raw_values_schema_version: i16,
    source_payload: Json<serde_json::Value>,
    raw_values: Json<serde_json::Value>,
    normalization_context: Json<serde_json::Value>,
    provenance: Json<serde_json::Value>,
    source_event_id: Option<String>,
    source_occurred_at: Option<OffsetDateTime>,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct ProviderReceiptRow {
    scope: String,
    delivery_id: String,
    source_payload_sha256: Vec<u8>,
}

async fn raw_revision(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
    revision: i64,
) -> RawRevisionRow {
    sqlx::query_as::<_, RawRevisionRow>(
        "SELECT r.operation, r.payload_format, r.raw_values_schema_version, r.source_payload, r.raw_values, r.normalization_context, r.provenance, r.source_event_id, r.source_occurred_at \
         FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams s ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.listing_source_id = $1 AND s.ingestion_method = 'SHOPIFY' AND s.source_record_key = $2 AND r.revision = $3",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .bind(revision)
    .fetch_one(&get_postgres_client().await)
    .await
    .map(|row| RawRevisionRow {
        operation: row.operation,
        payload_format: row.payload_format,
        raw_values_schema_version: row.raw_values_schema_version,
        source_payload: row.source_payload,
        raw_values: row.raw_values,
        normalization_context: row.normalization_context,
        provenance: row.provenance,
        source_event_id: row.source_event_id,
        source_occurred_at: row.source_occurred_at,
    })
    .unwrap_or_else(|error| panic!("failed loading raw Shopify revision: {error}"))
}

async fn provider_receipts(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
) -> Vec<ProviderReceiptRow> {
    sqlx::query_as::<_, ProviderReceiptRow>(
        "SELECT r.provider_scope AS scope, r.provider_delivery_id AS delivery_id, r.observation_sha256 AS source_payload_sha256 \
         FROM product_listing_raw_provider_observation_receipts r \
         JOIN product_listing_raw_streams s ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.ingestion_method = 'SHOPIFY' AND s.listing_source_id = $1 AND s.source_record_key = $2 \
         ORDER BY r.provider_scope, r.provider_delivery_id",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_all(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading Shopify provider receipts: {error}"))
}

fn source_payload_digest(payload: &serde_json::Value) -> Vec<u8> {
    SourcePayload::new(payload.clone())
        .and_then(|source_payload| source_payload.canonical_sha256())
        .map(|sha256| sha256.as_bytes().to_vec())
        .unwrap_or_else(|error| panic!("failed hashing Shopify source payload: {error}"))
}

fn occurred_at(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339)
        .unwrap_or_else(|error| panic!("invalid Shopify fixture timestamp: {error}"))
}

async fn raw_revision_count(listing_source_id: ListingSourceId, source_listing_id: u64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams s ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.ingestion_method = 'SHOPIFY' AND s.listing_source_id = $1 AND s.source_record_key = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed counting raw Shopify revisions: {error}"))
}

async fn raw_revision_count_for_source_listing_id(source_listing_id: u64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams s ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.ingestion_method = 'SHOPIFY' AND s.source_record_key = $1",
    )
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed counting raw Shopify revisions: {error}"))
}

async fn normalize_pending_shopify_revisions(pool: sqlx::PgPool) -> usize {
    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool),
    );
    normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::Reconcile,
            max_revisions_per_stream: 32,
            pending_stream_limit: 100,
        })
        .await
        .map(|result| result.revisions.len())
        .unwrap_or_else(|error| panic!("failed normalizing Shopify raw revision: {error}"))
}

async fn listing_price_amount(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT price_amount FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading normalized Shopify price: {error}"))
}

async fn listing_facts(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
) -> (Option<String>, i64, String, String) {
    sqlx::query_as(
        "SELECT availability, price_amount, price_currency, url FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading normalized Shopify listing: {error}"))
}

async fn listing_count(listing_source_id: ListingSourceId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1")
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_one(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed counting ProductListings: {error}"))
}

async fn product_listing_event_count() -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM product_listing_events")
        .fetch_one(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed counting ProductListing events: {error}"))
}

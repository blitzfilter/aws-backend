use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    seed_access_token_for, seed_listing_source, seed_operator_partnership_listing_source_grant,
    seed_partnership_membership, seed_user,
};
use base64::Engine;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use platform_postgres::SqlxUnitOfWork;
use product_listing_normalization::SourcePayload;
use product_listing_postgres::{
    SqlxPendingProductListingRawStreamReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRawNormalizationWriterFactory, SqlxProductListingRepositoryFactory,
};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionHandler,
    NormalizeProductListingRawRevisionMode, NormalizeProductListingRawRevisionUseCase,
};
use serde_json::json;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user_core::access_token::Scope;

const SECRET: &str = "woocommerce-webhook-test-secret";

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn assert_test_result(result: TestResult) {
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_capture_changed_woocommerce_product_after_signature_validation() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let body = json!({
            "id": 17,
            "name": "Woo Cabinet",
            "permalink": "https://partner.example/product-listings/woo-cabinet",
            "description": "<p>Cabinet description</p>",
            "price": "42.699",
            "status": "publish",
            "stock_status": "instock",
            "images": [],
            "futureWooKey": { "nested": true }
        })
        .to_string();

        let response = send(
            &listing_source_id,
            &token,
            "product.created",
            &body,
            Some("delivery-1"),
        )
        .await?;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert!(response.bytes().await?.is_empty());

        let pool = get_postgres_client().await;
        let row: (String, i16, serde_json::Value, serde_json::Value, Option<String>) = sqlx::query_as(
            "SELECT r.operation, r.raw_values_schema_version, r.source_payload, r.raw_values, r.source_event_id \
             FROM product_listing_raw_revisions r \
             JOIN product_listing_raw_streams s \
               ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
             WHERE s.listing_source_id = $1 AND s.ingestion_method = 'WOOCOMMERCE' \
               AND s.source_record_key = '17'",
        )
        .bind(uuid::Uuid::parse_str(&listing_source_id)?)
        .fetch_one(&pool)
        .await?;
        assert_eq!("UPSERT", row.0);
        assert_eq!(2, row.1);
        assert_eq!(json!(true), row.2["futureWooKey"]["nested"]);
        assert_eq!(json!("MACHINE_DECIMAL"), row.3["priceFormat"]);
        assert_eq!(
            json!({"action": "SET", "value": "in stock"}),
            row.3["availability"]
        );
        assert_eq!(Some("delivery-1".to_owned()), row.4);
        assert_eq!(
            0,
            product_count(uuid::Uuid::parse_str(&listing_source_id)?).await?
        );
        assert_eq!(
            0,
            product_listing_event_count(uuid::Uuid::parse_str(&listing_source_id)?).await?
        );
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_normalize_woocommerce_machine_decimal_prices_and_preserve_provider_strings() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let listing_source_uuid = uuid::Uuid::parse_str(&listing_source_id)?;
        let cases = [
            (28, "42.000", Some(4_200_i64)),
            (29, "42.5", Some(4_250_i64)),
            (30, "42.50", Some(4_250_i64)),
            (31, "", None),
        ];

        for &(product_id, price, _) in &cases {
            let source_record_key = product_id.to_string();
            let body = product_body(product_id, price, "publish", Some("instock"));
            let delivery_id = format!("machine-decimal-{product_id}");
            let response = send(
                &listing_source_id,
                &token,
                "product.created",
                &body,
                Some(&delivery_id),
            )
            .await?;

            assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
            let (raw_values_schema_version, source_payload, raw_values) =
                captured_raw_revision(listing_source_uuid, &source_record_key).await?;
            assert_eq!(2, raw_values_schema_version);
            assert_eq!(json!(price), source_payload["price"]);
            assert_eq!(json!("MACHINE_DECIMAL"), raw_values["priceFormat"]);
            let expected_price_patch = if price.is_empty() {
                json!({"action": "CLEAR"})
            } else {
                json!({"action": "SET", "value": price})
            };
            assert_eq!(expected_price_patch, raw_values["price"]);
        }

        assert_eq!(
            cases.len(),
            normalize_pending_woocommerce_revisions(get_postgres_client().await).await?
        );
        for &(product_id, _, expected_amount) in &cases {
            let source_record_key = product_id.to_string();
            assert_eq!(
                expected_amount,
                product_price_amount(listing_source_uuid, &source_record_key).await?
            );
        }
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_not_create_revision_when_only_woocommerce_delivery_id_changes() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let body = product_body(20, "42.00", "publish", Some("outofstock"));

        for delivery_id in ["delivery-one", "delivery-two"] {
            let response = send(
                &listing_source_id,
                &token,
                "product.updated",
                &body,
                Some(delivery_id),
            )
            .await?;
            assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        }

        let listing_source_id = uuid::Uuid::parse_str(&listing_source_id)?;
        assert_eq!(1, raw_revision_count(listing_source_id, "20").await?);
        assert_eq!(2, provider_receipt_count(listing_source_id, "20").await?);
        assert_eq!(
            (None, None),
            raw_stream_source_order(listing_source_id, "20").await?
        );
        assert_eq!(0, product_count(listing_source_id).await?);
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_capture_woocommerce_e1_a_e2_b_retry_e1_and_e3_a_in_provider_order() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let source_record_key = "26";
        let e1_timestamp = "2026-09-06T10:00:00";
        let e2_timestamp = "2026-09-06T10:01:00";
        let e3_timestamp = "2026-09-06T10:02:00";
        let e1 =
            product_body_with_modified_gmt(26, "42.00", "publish", Some("instock"), e1_timestamp);
        let e1_retry = reordered_product_body_with_modified_gmt(
            26,
            "42.00",
            "publish",
            Some("instock"),
            e1_timestamp,
        );
        let e2 =
            product_body_with_modified_gmt(26, "43.00", "publish", Some("instock"), e2_timestamp);
        let e3 =
            product_body_with_modified_gmt(26, "42.00", "publish", Some("instock"), e3_timestamp);
        assert_ne!(e1, e1_retry);
        assert_eq!(
            canonical_source_payload_digest(&e1)?,
            canonical_source_payload_digest(&e1_retry)?
        );

        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id,
                &token,
                "product.created",
                &e1,
                Some("delivery-e1"),
            )
            .await?
            .status()
        );
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id,
                &token,
                "product.updated",
                &e2,
                Some("delivery-e2"),
            )
            .await?
            .status()
        );

        let listing_source_id = uuid::Uuid::parse_str(&listing_source_id)?;
        assert_eq!(
            2,
            raw_revision_count(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            (
                Some(woocommerce_timestamp(e2_timestamp)?),
                Some(canonical_source_payload_digest(&e2)?),
            ),
            raw_stream_source_order(listing_source_id, source_record_key).await?
        );

        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id.to_string(),
                &token,
                "product.created",
                &e1_retry,
                Some("delivery-e1"),
            )
            .await?
            .status()
        );
        assert_eq!(
            2,
            raw_revision_count(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            (
                Some(woocommerce_timestamp(e2_timestamp)?),
                Some(canonical_source_payload_digest(&e2)?),
            ),
            raw_stream_source_order(listing_source_id, source_record_key).await?
        );

        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id.to_string(),
                &token,
                "product.updated",
                &e3,
                Some("delivery-e3"),
            )
            .await?
            .status()
        );
        assert_eq!(
            3,
            raw_revision_count(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            vec![
                (
                    1,
                    Some("delivery-e1".to_owned()),
                    Some(woocommerce_timestamp(e1_timestamp)?),
                    json!({"action": "SET", "value": "42.00"}),
                ),
                (
                    2,
                    Some("delivery-e2".to_owned()),
                    Some(woocommerce_timestamp(e2_timestamp)?),
                    json!({"action": "SET", "value": "43.00"}),
                ),
                (
                    3,
                    Some("delivery-e3".to_owned()),
                    Some(woocommerce_timestamp(e3_timestamp)?),
                    json!({"action": "SET", "value": "42.00"}),
                ),
            ],
            raw_revisions(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            vec![
                (
                    "product.created".to_owned(),
                    "delivery-e1".to_owned(),
                    canonical_source_payload_digest(&e1)?,
                ),
                (
                    "product.updated".to_owned(),
                    "delivery-e2".to_owned(),
                    canonical_source_payload_digest(&e2)?,
                ),
                (
                    "product.updated".to_owned(),
                    "delivery-e3".to_owned(),
                    canonical_source_payload_digest(&e3)?,
                ),
            ],
            provider_receipts(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            (
                Some(woocommerce_timestamp(e3_timestamp)?),
                Some(canonical_source_payload_digest(&e3)?),
            ),
            raw_stream_source_order(listing_source_id, source_record_key).await?
        );
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_acknowledge_unchanged_woocommerce_receipts_and_enforce_receipt_and_source_order() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let source_record_key = "27";
        let stale_timestamp = "2026-09-06T10:00:00";
        let current_timestamp = "2026-09-06T10:01:00";
        let newer_timestamp = "2026-09-06T10:02:00";
        let current = product_body_with_modified_gmt(
            27,
            "42.00",
            "publish",
            Some("instock"),
            current_timestamp,
        );
        let changed = product_body_with_modified_gmt(
            27,
            "43.00",
            "publish",
            Some("instock"),
            newer_timestamp,
        );
        let same_timestamp_changed = product_body_with_modified_gmt(
            27,
            "44.00",
            "publish",
            Some("instock"),
            current_timestamp,
        );
        let stale = product_body_with_modified_gmt(
            27,
            "45.00",
            "publish",
            Some("instock"),
            stale_timestamp,
        );
        let invalid_timestamp = product_body_with_modified_gmt(
            27,
            "46.00",
            "publish",
            Some("instock"),
            "not-a-gmt-timestamp",
        );
        let non_utc_offset_timestamp = product_body_with_modified_gmt(
            27,
            "47.00",
            "publish",
            Some("instock"),
            "2026-09-06T10:03:00+01:00",
        );

        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id,
                &token,
                "product.updated",
                &current,
                Some("delivery-original"),
            )
            .await?
            .status()
        );
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id,
                &token,
                "product.updated",
                &current,
                Some("delivery-unchanged"),
            )
            .await?
            .status()
        );
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id,
                &token,
                "product.updated",
                &current,
                Some("delivery-unchanged"),
            )
            .await?
            .status()
        );

        let listing_source_id = uuid::Uuid::parse_str(&listing_source_id)?;
        assert_eq!(
            1,
            raw_revision_count(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            2,
            provider_receipt_count(listing_source_id, source_record_key).await?
        );

        let receipt_conflict = send(
            &listing_source_id.to_string(),
            &token,
            "product.updated",
            &changed,
            Some("delivery-unchanged"),
        )
        .await?;
        assert_eq!(reqwest::StatusCode::CONFLICT, receipt_conflict.status());
        assert_eq!(
            "WOOCOMMERCE_PROVIDER_RECEIPT_DIGEST_CONFLICT",
            receipt_conflict.json::<serde_json::Value>().await?["error"]
        );

        let source_order_conflict = send(
            &listing_source_id.to_string(),
            &token,
            "product.updated",
            &same_timestamp_changed,
            Some("delivery-source-order-conflict"),
        )
        .await?;
        assert_eq!(
            reqwest::StatusCode::CONFLICT,
            source_order_conflict.status()
        );
        assert_eq!(
            "WOOCOMMERCE_PROVIDER_SOURCE_ORDER_CONFLICT",
            source_order_conflict.json::<serde_json::Value>().await?["error"]
        );

        for (body, delivery_id) in [
            (&invalid_timestamp, "delivery-invalid-timestamp"),
            (
                &non_utc_offset_timestamp,
                "delivery-non-utc-offset-timestamp",
            ),
        ] {
            let invalid_timestamp_response = send(
                &listing_source_id.to_string(),
                &token,
                "product.updated",
                body,
                Some(delivery_id),
            )
            .await?;
            assert_eq!(
                reqwest::StatusCode::BAD_REQUEST,
                invalid_timestamp_response.status()
            );
            assert_eq!(
                "BAD_BODY_VALUE",
                invalid_timestamp_response
                    .json::<serde_json::Value>()
                    .await?["error"]
            );
        }

        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id.to_string(),
                &token,
                "product.updated",
                &stale,
                Some("delivery-stale"),
            )
            .await?
            .status()
        );
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id.to_string(),
                &token,
                "product.updated",
                &stale,
                Some("delivery-stale"),
            )
            .await?
            .status()
        );

        assert_eq!(
            1,
            raw_revision_count(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            3,
            provider_receipt_count(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            vec![
                (
                    "product.updated".to_owned(),
                    "delivery-original".to_owned(),
                    canonical_source_payload_digest(&current)?,
                ),
                (
                    "product.updated".to_owned(),
                    "delivery-stale".to_owned(),
                    canonical_source_payload_digest(&stale)?,
                ),
                (
                    "product.updated".to_owned(),
                    "delivery-unchanged".to_owned(),
                    canonical_source_payload_digest(&current)?,
                ),
            ],
            provider_receipts(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            (
                Some(woocommerce_timestamp(current_timestamp)?),
                Some(canonical_source_payload_digest(&current)?),
            ),
            raw_stream_source_order(listing_source_id, source_record_key).await?
        );
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_capture_changed_unknown_woocommerce_payload_key() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let first = json!({
            "id": 21,
            "name": "Woo Cabinet",
            "permalink": "https://partner.example/product-listings/woo-cabinet-21",
            "price": "42.00",
            "status": "publish",
            "stock_status": "onbackorder",
            "images": [],
            "futureWooKey": "first"
        })
        .to_string();
        let second = first.replace("\"first\"", "\"second\"");

        for (body, delivery_id) in [(&first, "delivery-one"), (&second, "delivery-two")] {
            let response = send(
                &listing_source_id,
                &token,
                "product.updated",
                body,
                Some(delivery_id),
            )
            .await?;
            assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        }

        let pool = get_postgres_client().await;
        let listing_source_id = uuid::Uuid::parse_str(&listing_source_id)?;
        assert_eq!(2, raw_revision_count(listing_source_id, "21").await?);
        let availability: serde_json::Value = sqlx::query_scalar(
            "SELECT r.raw_values -> 'availability' \
             FROM product_listing_raw_revisions r \
             JOIN product_listing_raw_streams s \
               ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
             WHERE s.listing_source_id = $1 AND s.source_record_key = '21' \
             ORDER BY r.revision DESC LIMIT 1",
        )
        .bind(listing_source_id)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            json!({"action": "SET", "value": "https://schema.org/BackOrder"}),
            availability
        );
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_capture_delete_before_asynchronous_withdrawal() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let created = product_body(22, "42.00", "publish", Some("instock"));
        let deleted = json!({ "id": 22 }).to_string();
        for (topic, body) in [("product.created", &created), ("product.deleted", &deleted)] {
            let response = send(&listing_source_id, &token, topic, body, None).await?;
            assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        }
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            send(
                &listing_source_id,
                &token,
                "product.deleted",
                &deleted,
                None
            )
            .await?
            .status()
        );

        let pool = get_postgres_client().await;
        let operations: Vec<String> = sqlx::query_scalar(
            "SELECT r.operation \
             FROM product_listing_raw_revisions r \
             JOIN product_listing_raw_streams s \
               ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
             WHERE s.listing_source_id = $1 AND s.source_record_key = '22' \
             ORDER BY r.revision",
        )
        .bind(uuid::Uuid::parse_str(&listing_source_id)?)
        .fetch_all(&pool)
        .await?;
        assert_eq!(vec!["UPSERT", "DELETE"], operations);
        let listing_source_id = uuid::Uuid::parse_str(&listing_source_id)?;
        assert_eq!(0, provider_receipt_count(listing_source_id, "22").await?);
        assert_eq!(
            (None, None),
            raw_stream_source_order(listing_source_id, "22").await?
        );
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_not_capture_woocommerce_webhook_with_invalid_signature() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let body = json!({ "id": 23 }).to_string();
        let response = reqwest::Client::new()
            .post(format!(
                "{}/api/v1/webhooks/woocommerce/{listing_source_id}",
                AURA_API.base_url()
            ))
            .bearer_auth(token)
            .header("x-wc-webhook-topic", "product.deleted")
            .header("x-wc-webhook-signature", signature("different-body"))
            .body(body)
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::UNAUTHORIZED, response.status());
        assert_eq!(
            "BAD_HEADER_VALUE",
            response.json::<serde_json::Value>().await?["error"]
        );
        assert_eq!(
            0,
            raw_revision_count(uuid::Uuid::parse_str(&listing_source_id)?, "23").await?
        );
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_published_deleted_or_ignored_woocommerce_webhooks_without_product_write_capability()
 {
    let result: TestResult = async {
        let listing_source = seed_listing_source().await;
        configure_woocommerce_source(listing_source).await?;
        let user_id = seed_user("USER").await;
        seed_partnership_membership(user_id, listing_source).await;
        seed_operator_partnership_listing_source_grant(listing_source).await;
        let token = String::from(seed_access_token_for(user_id, HashSet::new()).await);
        let cases = [
            (
                "published",
                "product.updated",
                "24",
                product_body(24, "42.00", "publish", Some("instock")),
            ),
            (
                "deleted",
                "product.deleted",
                "25",
                json!({ "id": 25 }).to_string(),
            ),
            (
                "ignored",
                "product.updated",
                "26",
                json!({ "id": 26, "status": "future-status" }).to_string(),
            ),
        ];

        for (case, topic, source_record_key, body) in cases {
            let response = send(&listing_source.to_string(), &token, topic, &body, None).await?;

            assert_eq!(reqwest::StatusCode::FORBIDDEN, response.status(), "{case}");
            assert_eq!(
                "FORBIDDEN",
                response.json::<serde_json::Value>().await?["error"],
                "{case}"
            );
            assert_eq!(
                0,
                raw_revision_count(listing_source, source_record_key).await?,
                "{case}"
            );
        }
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_published_deleted_or_ignored_woocommerce_webhooks_when_write_capable_caller_lacks_listing_source_grant()
 {
    let result: TestResult = async {
        let listing_source = seed_listing_source().await;
        configure_woocommerce_source(listing_source).await?;
        seed_operator_partnership_listing_source_grant(listing_source).await;
        let user_id = seed_user("USER").await;
        let token = String::from(
            seed_access_token_for(user_id, HashSet::from([Scope::ProductListingsWrite])).await,
        );
        let cases = [
            (
                "published",
                "product.updated",
                "27",
                product_body(27, "42.00", "publish", Some("instock")),
            ),
            (
                "deleted",
                "product.deleted",
                "28",
                json!({ "id": 28 }).to_string(),
            ),
            (
                "ignored",
                "product.updated",
                "29",
                json!({ "id": 29, "status": "future-status" }).to_string(),
            ),
        ];

        for (case, topic, source_record_key, body) in cases {
            let response = send(&listing_source.to_string(), &token, topic, &body, None).await?;

            assert_eq!(reqwest::StatusCode::FORBIDDEN, response.status(), "{case}");
            assert_eq!(
                "FORBIDDEN",
                response.json::<serde_json::Value>().await?["error"],
                "{case}"
            );
            assert_eq!(
                0,
                raw_revision_count(listing_source, source_record_key).await?,
                "{case}"
            );
        }
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_acknowledge_authorized_signed_ignored_woocommerce_webhook_without_raw_capture_or_provider_receipt()
 {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        let source_record_key = "30";
        let body = json!({ "id": 30, "status": "future-status" }).to_string();

        let response = send(
            &listing_source_id,
            &token,
            "product.updated",
            &body,
            Some("ignored-delivery-30"),
        )
        .await?;

        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert!(response.bytes().await?.is_empty());
        let listing_source_id = uuid::Uuid::parse_str(&listing_source_id)?;
        assert_eq!(
            0,
            raw_revision_count(listing_source_id, source_record_key).await?
        );
        assert_eq!(
            0,
            provider_receipt_count(listing_source_id, source_record_key).await?
        );
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_missing_or_malformed_woocommerce_webhook_without_capture() {
    let result: TestResult = async {
        let (listing_source_id, token) = webhook_auth().await?;
        for (topic, body, expected_error) in [
            (
                "orders.created",
                json!({ "id": 25 }).to_string(),
                "BAD_HEADER_VALUE",
            ),
            ("product.created", "not-json".to_owned(), "BAD_BODY_VALUE"),
            ("product.created", "".to_owned(), "BAD_BODY_VALUE"),
        ] {
            let response = send(&listing_source_id, &token, topic, &body, None).await?;
            assert_eq!(reqwest::StatusCode::BAD_REQUEST, response.status());
            assert_eq!(
                expected_error,
                response.json::<serde_json::Value>().await?["error"]
            );
        }
        assert_eq!(
            0,
            raw_revision_count(uuid::Uuid::parse_str(&listing_source_id)?, "25").await?
        );
        Ok(())
    }
    .await;
    assert_test_result(result);
}

async fn webhook_auth() -> Result<(String, String), Box<dyn std::error::Error>> {
    let listing_source = seed_listing_source().await;
    let listing_source_id = listing_source.to_string();
    configure_woocommerce_source(listing_source).await?;
    let user_id = seed_user("USER").await;
    seed_partnership_membership(user_id, listing_source).await;
    seed_operator_partnership_listing_source_grant(listing_source).await;
    let token = seed_access_token_for(user_id, HashSet::from([Scope::ProductListingsWrite])).await;
    Ok((listing_source_id, String::from(token)))
}

async fn configure_woocommerce_source(listing_source_id: uuid::Uuid) -> Result<(), sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query(
        "INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'WOOCOMMERCE')",
    )
    .bind(listing_source_id)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO listing_source_woocommerce_ingestion_configurations (listing_source_id, webhook_secret, currency, language) VALUES ($1, $2, 'EUR', 'en')",
    )
    .bind(listing_source_id)
    .bind(SECRET)
    .execute(&pool)
    .await?;
    Ok(())
}

async fn captured_raw_revision(
    listing_source_id: uuid::Uuid,
    source_record_key: &str,
) -> Result<(i16, serde_json::Value, serde_json::Value), sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_as(
        "SELECT r.raw_values_schema_version, r.source_payload, r.raw_values \
         FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams stream \
           ON stream.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE stream.listing_source_id = $1 AND stream.ingestion_method = 'WOOCOMMERCE' \
           AND stream.source_record_key = $2",
    )
    .bind(listing_source_id)
    .bind(source_record_key)
    .fetch_one(&pool)
    .await
}

async fn raw_revision_count(
    listing_source_id: uuid::Uuid,
    source_record_key: &str,
) -> Result<i64, sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_scalar(
        "SELECT COUNT(*) \
         FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams s \
           ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.listing_source_id = $1 AND s.ingestion_method = 'WOOCOMMERCE' \
           AND s.source_record_key = $2",
    )
    .bind(listing_source_id)
    .bind(source_record_key)
    .fetch_one(&pool)
    .await
}

async fn provider_receipt_count(
    listing_source_id: uuid::Uuid,
    source_record_key: &str,
) -> Result<i64, sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_scalar(
        "SELECT COUNT(*) \
         FROM product_listing_raw_provider_observation_receipts receipt \
         JOIN product_listing_raw_streams stream \
           ON stream.product_listing_raw_stream_id = receipt.product_listing_raw_stream_id \
         WHERE stream.listing_source_id = $1 AND stream.ingestion_method = 'WOOCOMMERCE' \
           AND stream.source_record_key = $2",
    )
    .bind(listing_source_id)
    .bind(source_record_key)
    .fetch_one(&pool)
    .await
}

async fn provider_receipts(
    listing_source_id: uuid::Uuid,
    source_record_key: &str,
) -> Result<Vec<(String, String, Vec<u8>)>, sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_as(
        "SELECT receipt.provider_scope, receipt.provider_delivery_id, receipt.observation_sha256 \
         FROM product_listing_raw_provider_observation_receipts receipt \
         JOIN product_listing_raw_streams stream \
           ON stream.product_listing_raw_stream_id = receipt.product_listing_raw_stream_id \
         WHERE stream.listing_source_id = $1 AND stream.ingestion_method = 'WOOCOMMERCE' \
           AND stream.source_record_key = $2 \
         ORDER BY receipt.provider_scope, receipt.provider_delivery_id",
    )
    .bind(listing_source_id)
    .bind(source_record_key)
    .fetch_all(&pool)
    .await
}

async fn raw_stream_source_order(
    listing_source_id: uuid::Uuid,
    source_record_key: &str,
) -> Result<(Option<OffsetDateTime>, Option<Vec<u8>>), sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_as(
        "SELECT latest_provider_source_occurred_at, latest_provider_source_observation_sha256 \
         FROM product_listing_raw_streams \
         WHERE listing_source_id = $1 AND ingestion_method = 'WOOCOMMERCE' \
           AND source_record_key = $2",
    )
    .bind(listing_source_id)
    .bind(source_record_key)
    .fetch_one(&pool)
    .await
}

async fn raw_revisions(
    listing_source_id: uuid::Uuid,
    source_record_key: &str,
) -> Result<
    Vec<(
        i64,
        Option<String>,
        Option<OffsetDateTime>,
        serde_json::Value,
    )>,
    sqlx::Error,
> {
    let pool = get_postgres_client().await;
    sqlx::query_as(
        "SELECT r.revision, r.source_event_id, r.source_occurred_at, r.raw_values -> 'price' \
         FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams stream \
           ON stream.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE stream.listing_source_id = $1 AND stream.ingestion_method = 'WOOCOMMERCE' \
           AND stream.source_record_key = $2 \
         ORDER BY r.revision",
    )
    .bind(listing_source_id)
    .bind(source_record_key)
    .fetch_all(&pool)
    .await
}

async fn normalize_pending_woocommerce_revisions(
    pool: sqlx::PgPool,
) -> Result<usize, Box<dyn std::error::Error>> {
    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool),
    );
    let result = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::Reconcile,
            max_revisions_per_stream: 32,
            pending_stream_limit: 100,
        })
        .await?;
    Ok(result.revisions.len())
}

async fn product_price_amount(
    listing_source_id: uuid::Uuid,
    source_listing_id: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_scalar(
        "SELECT price_amount FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(listing_source_id)
    .bind(source_listing_id)
    .fetch_one(&pool)
    .await
}

async fn product_count(listing_source_id: uuid::Uuid) -> Result<i64, sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_scalar("SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1")
        .bind(listing_source_id)
        .fetch_one(&pool)
        .await
}

async fn product_listing_event_count(listing_source_id: uuid::Uuid) -> Result<i64, sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query_scalar(
        "SELECT COUNT(*) \
         FROM product_listing_events e \
         JOIN product_listings p ON p.product_listing_id = e.product_listing_id \
         WHERE p.listing_source_id = $1",
    )
    .bind(listing_source_id)
    .fetch_one(&pool)
    .await
}

fn product_body(id: u64, price: &str, status: &str, stock_status: Option<&str>) -> String {
    product_body_with_optional_modified_gmt(id, price, status, stock_status, None)
}

fn product_body_with_modified_gmt(
    id: u64,
    price: &str,
    status: &str,
    stock_status: Option<&str>,
    date_modified_gmt: &str,
) -> String {
    product_body_with_optional_modified_gmt(
        id,
        price,
        status,
        stock_status,
        Some(date_modified_gmt),
    )
}

fn product_body_with_optional_modified_gmt(
    id: u64,
    price: &str,
    status: &str,
    stock_status: Option<&str>,
    date_modified_gmt: Option<&str>,
) -> String {
    let mut body = json!({
        "id": id,
        "name": "Woo Cabinet",
        "permalink": format!("https://partner.example/product-listings/woo-cabinet-{id}"),
        "description": "<p>Cabinet description</p>",
        "price": price,
        "status": status,
        "stock_status": stock_status,
        "images": []
    });
    if let Some(date_modified_gmt) = date_modified_gmt {
        body["date_modified_gmt"] = json!(date_modified_gmt);
    }
    body.to_string()
}

fn reordered_product_body_with_modified_gmt(
    id: u64,
    price: &str,
    status: &str,
    stock_status: Option<&str>,
    date_modified_gmt: &str,
) -> String {
    let stock_status = stock_status
        .map(|value| format!("\"{value}\""))
        .unwrap_or_else(|| "null".to_owned());
    format!(
        r#"{{"id":{id},"name":"Woo Cabinet","permalink":"https://partner.example/product-listings/woo-cabinet-{id}","description":"<p>Cabinet description</p>","price":"{price}","status":"{status}","stock_status":{stock_status},"images":[],"date_modified_gmt":"{date_modified_gmt}"}}"#
    )
}

fn canonical_source_payload_digest(body: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let payload = SourcePayload::new(serde_json::from_str(body)?)?;
    Ok(payload.canonical_sha256()?.as_bytes().to_vec())
}

fn woocommerce_timestamp(value: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(&format!("{value}Z"), &Rfc3339)
}

async fn send(
    listing_source_id: &str,
    token: &str,
    topic: &str,
    body: &str,
    delivery_id: Option<&str>,
) -> Result<reqwest::Response, reqwest::Error> {
    let request = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/webhooks/woocommerce/{listing_source_id}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .header("x-wc-webhook-topic", topic)
        .header("x-wc-webhook-signature", signature(body));
    let request = match delivery_id {
        Some(delivery_id) => request.header("x-wc-webhook-delivery-id", delivery_id),
        None => request,
    };
    request.body(body.to_owned()).send().await
}

fn signature(body: &str) -> String {
    let key = PKey::hmac(SECRET.as_bytes())
        .unwrap_or_else(|error| panic!("failed creating HMAC key: {error}"));
    let mut signer = Signer::new(MessageDigest::sha256(), &key)
        .unwrap_or_else(|error| panic!("failed creating HMAC signer: {error}"));
    signer
        .update(body.as_bytes())
        .unwrap_or_else(|error| panic!("failed signing webhook body: {error}"));
    base64::engine::general_purpose::STANDARD.encode(
        signer
            .sign_to_vec()
            .unwrap_or_else(|error| panic!("failed finalizing HMAC signature: {error}")),
    )
}

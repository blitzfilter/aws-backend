use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    seed_access_token_for, seed_listing_source, seed_operator_partnership_listing_source_grant,
    seed_partnership_membership, seed_user,
};
use base64::Engine;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use serde_json::json;
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
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
        let row: (String, serde_json::Value, serde_json::Value, Option<String>) = sqlx::query_as(
            "SELECT r.operation, r.source_payload, r.raw_values, r.source_event_id \
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
        assert_eq!(json!(true), row.1["futureWooKey"]["nested"]);
        assert_eq!(
            json!({"action": "SET", "value": "in stock"}),
            row.2["availability"]
        );
        assert_eq!(Some("delivery-1".to_owned()), row.3);
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
        assert_eq!(0, product_count(listing_source_id).await?);
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
            .header("x-wc-webhook-signature", "invalid")
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
async fn should_reject_woocommerce_webhook_without_product_write_capability() {
    let result: TestResult = async {
        let listing_source = seed_listing_source().await;
        configure_woocommerce_source(listing_source).await?;
        let user_id = seed_user("USER").await;
        seed_partnership_membership(user_id, listing_source).await;
        seed_operator_partnership_listing_source_grant(listing_source).await;
        let token = String::from(seed_access_token_for(user_id, HashSet::new()).await);
        let body = json!({ "id": 24 }).to_string();

        let response = send(
            &listing_source.to_string(),
            &token,
            "product.deleted",
            &body,
            None,
        )
        .await?;
        assert_eq!(reqwest::StatusCode::FORBIDDEN, response.status());
        assert_eq!(
            "FORBIDDEN",
            response.json::<serde_json::Value>().await?["error"]
        );
        assert_eq!(0, raw_revision_count(listing_source, "24").await?);
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
    json!({
        "id": id,
        "name": "Woo Cabinet",
        "permalink": format!("https://partner.example/product-listings/woo-cabinet-{id}"),
        "description": "<p>Cabinet description</p>",
        "price": price,
        "status": status,
        "stock_status": stock_status,
        "images": []
    })
    .to_string()
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

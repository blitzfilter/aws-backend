//! Versioned SQS boundary. Additive unknown envelope/payload fields are deliberately ignored.
//! Required fields, discriminators, IDs and keys remain strict; CDC event payloads are separate.
use crate::{
    WorkerScope,
    cdc::CdcOperation,
    jobs::{
        DomainJob, DomainJobPayload, IdempotencyKey, InvalidJob, NotificationDeliveryCreatedJob,
        OrderingKey, ProductListingEventJob, ProductListingRawRevisionJob, SearchFilterChangedJob,
        SearchFilterMatchCreatedJob, canonical_uuid,
    },
};
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

pub(crate) const MAX_JOB_BYTES: usize = 16 * 1024;

#[derive(Serialize, Deserialize)]
struct EnvelopeV1 {
    schema_version: u32,
    scope: String,
    idempotency_key: String,
    ordering_key: String,
    #[serde(flatten)]
    job: PayloadV1,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "job_type", content = "payload")]
enum PayloadV1 {
    #[serde(rename = "PRODUCT_LISTING_EVENT")]
    ProductListingEvent {
        event_id: String,
        product_listing_id: String,
    },
    #[serde(rename = "PRODUCT_LISTING_RAW_REVISION")]
    ProductListingRawRevision {
        product_listing_raw_stream_id: String,
        product_listing_raw_revision_id: String,
        revision: u64,
    },
    #[serde(rename = "SEARCH_FILTER_CHANGED")]
    SearchFilterChanged {
        user_id: String,
        user_search_filter_id: String,
        version: i64,
        operation: OperationV1,
    },
    #[serde(rename = "SEARCH_FILTER_MATCH_CREATED")]
    SearchFilterMatchCreated {
        user_id: String,
        user_search_filter_id: String,
        product_listing_id: String,
        origin_event_id: String,
    },
    #[serde(rename = "NOTIFICATION_DELIVERY_CREATED")]
    NotificationDeliveryCreated { notification_delivery_id: String },
}

#[derive(Serialize, Deserialize)]
enum OperationV1 {
    #[serde(rename = "INSERT")]
    Insert,
    #[serde(rename = "UPDATE")]
    Update,
    #[serde(rename = "DELETE")]
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum WireError {
    #[error("invalid job JSON")]
    Json,
    #[error("unsupported job schema version")]
    SchemaVersion,
    #[error("job scope mismatch")]
    Scope,
    #[error("invalid job metadata")]
    Invalid,
    #[error("job exceeds size limit")]
    TooLarge,
}
impl From<InvalidJob> for WireError {
    fn from(_: InvalidJob) -> Self {
        Self::Invalid
    }
}

pub(crate) fn encode(job: &DomainJob) -> Result<String, WireError> {
    let scope = job.validate()?;
    let payload = match &job.payload {
        DomainJobPayload::ProductListingEvent(event) => PayloadV1::ProductListingEvent {
            event_id: event.event_id.to_string(),
            product_listing_id: event.product_listing_id.to_string(),
        },
        DomainJobPayload::ProductListingRawRevision(revision) => {
            PayloadV1::ProductListingRawRevision {
                product_listing_raw_stream_id: revision
                    .product_listing_raw_stream_id
                    .as_uuid()
                    .to_string(),
                product_listing_raw_revision_id: revision
                    .product_listing_raw_revision_id
                    .as_uuid()
                    .to_string(),
                revision: revision.revision,
            }
        }
        DomainJobPayload::SearchFilterChanged(change) => PayloadV1::SearchFilterChanged {
            user_id: change.user_id.clone(),
            user_search_filter_id: change.user_search_filter_id.clone(),
            version: change.version,
            operation: match change.operation {
                CdcOperation::Insert => OperationV1::Insert,
                CdcOperation::Update => OperationV1::Update,
                CdcOperation::Delete => OperationV1::Delete,
            },
        },
        DomainJobPayload::SearchFilterMatchCreated(change) => PayloadV1::SearchFilterMatchCreated {
            user_id: change.user_id.clone(),
            user_search_filter_id: change.user_search_filter_id.clone(),
            product_listing_id: change.product_listing_id.clone(),
            origin_event_id: change.origin_event_id.clone(),
        },
        DomainJobPayload::NotificationDeliveryCreated(delivery) => {
            PayloadV1::NotificationDeliveryCreated {
                notification_delivery_id: delivery.notification_delivery_id.clone(),
            }
        }
        DomainJobPayload::UserTierChanged(_) => return Err(WireError::Scope),
    };
    let encoded = serde_json::to_string(&EnvelopeV1 {
        schema_version: 1,
        scope: scope.as_str().to_owned(),
        idempotency_key: job.idempotency_key.as_str().to_owned(),
        ordering_key: job.ordering_key.as_str().to_owned(),
        job: payload,
    })
    .map_err(|_| WireError::Json)?;
    if encoded.len() > MAX_JOB_BYTES {
        return Err(WireError::TooLarge);
    }
    Ok(encoded)
}

pub(crate) fn decode(body: &str, expected_scope: WorkerScope) -> Result<DomainJob, WireError> {
    if body.len() > MAX_JOB_BYTES {
        return Err(WireError::TooLarge);
    }
    let envelope: EnvelopeV1 = serde_json::from_str(body).map_err(|_| WireError::Json)?;
    if envelope.schema_version != 1 {
        return Err(WireError::SchemaVersion);
    }
    let scope = WorkerScope::iter()
        .find(|scope| scope.as_str() == envelope.scope)
        .ok_or(WireError::Scope)?;
    if scope != expected_scope {
        return Err(WireError::Scope);
    }
    let payload = match envelope.job {
        PayloadV1::ProductListingEvent {
            event_id,
            product_listing_id,
        } => {
            canonical_uuid(&event_id)?;
            canonical_uuid(&product_listing_id)?;
            DomainJobPayload::ProductListingEvent(ProductListingEventJob {
                event_id: event_id
                    .as_str()
                    .try_into()
                    .map_err(|_| WireError::Invalid)?,
                product_listing_id: product_listing_id
                    .as_str()
                    .try_into()
                    .map_err(|_| WireError::Invalid)?,
            })
        }
        PayloadV1::ProductListingRawRevision {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        } => DomainJobPayload::ProductListingRawRevision(ProductListingRawRevisionJob {
            product_listing_raw_stream_id:
                product_listing_service::ports::ProductListingRawStreamId::from_uuid(
                    canonical_uuid(&product_listing_raw_stream_id)?,
                ),
            product_listing_raw_revision_id:
                product_listing_service::ports::ProductListingRawRevisionId::from_uuid(
                    canonical_uuid(&product_listing_raw_revision_id)?,
                ),
            revision,
        }),
        PayloadV1::SearchFilterChanged {
            user_id,
            user_search_filter_id,
            version,
            operation,
        } => DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
            user_id,
            user_search_filter_id,
            version,
            operation: match operation {
                OperationV1::Insert => CdcOperation::Insert,
                OperationV1::Update => CdcOperation::Update,
                OperationV1::Delete => CdcOperation::Delete,
            },
        }),
        PayloadV1::SearchFilterMatchCreated {
            user_id,
            user_search_filter_id,
            product_listing_id,
            origin_event_id,
        } => DomainJobPayload::SearchFilterMatchCreated(SearchFilterMatchCreatedJob {
            user_id,
            user_search_filter_id,
            product_listing_id,
            origin_event_id,
        }),
        PayloadV1::NotificationDeliveryCreated {
            notification_delivery_id,
        } => DomainJobPayload::NotificationDeliveryCreated(NotificationDeliveryCreatedJob {
            notification_delivery_id,
        }),
    };
    let job = DomainJob {
        target_queue: scope.consumer_queue(),
        idempotency_key: IdempotencyKey::new(envelope.idempotency_key),
        ordering_key: OrderingKey::new(envelope.ordering_key),
        payload,
    };
    job.validate()?;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::{MAX_JOB_BYTES, WireError, decode, encode};
    use crate::WorkerScope;
    use serde_json::{Value, json};
    use strum::IntoEnumIterator;

    const A: &str = "10000000-0000-0000-0000-000000000001";
    const B: &str = "20000000-0000-0000-0000-000000000002";
    const C: &str = "30000000-0000-0000-0000-000000000003";
    const D: &str = "40000000-0000-0000-0000-000000000004";

    fn snapshots() -> Vec<(WorkerScope, Value)> {
        let mut snapshots = vec![];
        for scope in WorkerScope::iter() {
            let (kind, payload, key, ordering) = match scope {
                WorkerScope::ProductListingRawNormalization => (
                    "PRODUCT_LISTING_RAW_REVISION",
                    json!({"product_listing_raw_stream_id": A, "product_listing_raw_revision_id": B, "revision": 3}),
                    format!("product-listing-raw-revision:{B}"),
                    format!("product-listing-raw-stream:{A}"),
                ),
                WorkerScope::SearchFilterProjection => (
                    "SEARCH_FILTER_CHANGED",
                    json!({"user_id": A, "user_search_filter_id": B, "version": 3, "operation": "UPDATE"}),
                    format!("search-filter:{B}:3:update"),
                    format!("search-filter:{B}"),
                ),
                WorkerScope::SearchFilterMatchNotification => (
                    "SEARCH_FILTER_MATCH_CREATED",
                    json!({"user_id": A, "user_search_filter_id": B, "product_listing_id": C, "origin_event_id": D}),
                    format!("search-filter-match:{A}:{B}:{C}:{D}"),
                    format!("user:{A}"),
                ),
                WorkerScope::NotificationDelivery => (
                    "NOTIFICATION_DELIVERY_CREATED",
                    json!({"notification_delivery_id": A}),
                    format!("notification-delivery:{A}"),
                    format!("notification-delivery:{A}"),
                ),
                _ => (
                    "PRODUCT_LISTING_EVENT",
                    json!({"event_id": A, "product_listing_id": B}),
                    format!("product-event:{A}"),
                    format!("product:{B}"),
                ),
            };
            snapshots.push((
                scope,
                json!({"schema_version": 1, "scope": scope.as_str(), "job_type": kind,
                "idempotency_key": key, "ordering_key": ordering, "payload": payload}),
            ));
        }
        snapshots
    }

    #[test]
    fn should_match_v1_snapshots_for_every_scope_and_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        for (scope, snapshot) in snapshots() {
            let job = decode(&snapshot.to_string(), scope)?;
            assert_eq!(snapshot, serde_json::from_str::<Value>(&encode(&job)?)?);
        }
        for (wire, key) in [
            ("INSERT", "insert"),
            ("UPDATE", "update"),
            ("DELETE", "delete"),
        ] {
            let (_, mut snapshot) = snapshots().remove(0);
            snapshot["payload"]["operation"] = json!(wire);
            snapshot["idempotency_key"] = json!(format!("search-filter:{B}:3:{key}"));
            let job = decode(&snapshot.to_string(), WorkerScope::SearchFilterProjection)?;
            assert_eq!(snapshot, serde_json::from_str::<Value>(&encode(&job)?)?);
        }
        Ok(())
    }

    #[test]
    fn should_accept_additive_unknown_fields_explicitly_for_all_variants()
    -> Result<(), Box<dyn std::error::Error>> {
        for (scope, mut value) in snapshots() {
            let expected = decode(&value.to_string(), scope)?;
            value["future_metadata"] = json!({"extra": [1, 2]});
            value["payload"]["future_metadata"] = json!(true);
            assert_eq!(expected, decode(&value.to_string(), scope)?);
        }
        Ok(())
    }

    fn invalid_payload_values(field: &str, value: &Value) -> Vec<Value> {
        if field.ends_with("id") {
            vec![
                json!("bad"),
                json!("00000000-0000-0000-0000-000000000000"),
                json!("10000000000000000000000000000001"),
                json!(3),
                Value::Null,
            ]
        } else if value.is_number() {
            vec![json!(0), json!(-1), json!(1.5), json!("3"), json!(u64::MAX)]
        } else {
            vec![json!("update"), json!("UNKNOWN"), Value::Null]
        }
    }

    #[test]
    fn should_reject_invalid_envelopes_and_payloads_for_every_variant() {
        for (scope, original) in snapshots() {
            for (field, value) in [
                ("schema_version", json!(0)),
                ("schema_version", json!(2)),
                ("schema_version", json!("1")),
                ("scope", json!("user-tier-enforcement")),
                ("scope", json!("SEARCH_FILTER_PROJECTION")),
                ("job_type", json!("unknown")),
                ("idempotency_key", json!("forged")),
                ("ordering_key", json!("forged")),
            ] {
                let mut body = original.clone();
                body[field] = value;
                assert!(decode(&body.to_string(), scope).is_err(), "{field}");
            }
            for field in original.as_object().unwrap().keys() {
                let mut body = original.clone();
                body.as_object_mut().unwrap().remove(field);
                assert!(decode(&body.to_string(), scope).is_err(), "missing {field}");
            }
            for (field, value) in original["payload"].as_object().unwrap() {
                let mut body = original.clone();
                body["payload"].as_object_mut().unwrap().remove(field);
                assert!(decode(&body.to_string(), scope).is_err(), "missing {field}");
                let invalid = invalid_payload_values(field, value);
                for value in invalid {
                    let mut body = original.clone();
                    body["payload"][field] = value;
                    assert!(decode(&body.to_string(), scope).is_err(), "invalid {field}");
                }
            }
            for wrong_scope in WorkerScope::iter().filter(|other| *other != scope) {
                assert!(decode(&original.to_string(), wrong_scope).is_err());
            }
        }
        assert_eq!(
            Err(WireError::TooLarge),
            decode(
                &" ".repeat(MAX_JOB_BYTES + 1),
                WorkerScope::NotificationDelivery
            )
        );
    }
}

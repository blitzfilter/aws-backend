//! Versioned SQS boundary. Additive unknown envelope/payload fields are deliberately ignored.
//! Required fields, discriminators, IDs and keys remain strict; CDC event payloads are separate.
use crate::{
    WorkerScope,
    cdc::CdcOperation,
    jobs::{
        DomainJob, DomainJobPayload, IdempotencyKey, InvalidJob, NotificationDeliveryCreatedJob,
        OrderingKey, ProductListingEventJob, ProductListingRawRevisionJob, SearchFilterChangedJob,
        SearchFilterMatchCreatedJob,
    },
};
use domain_primitives::event_id::EventId;
use notification_core::notification_delivery_id::NotificationDeliveryId;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::{ProductListingRawRevisionId, ProductListingRawStreamId};
use search_filter_core::user_search_filter_id::UserSearchFilterId;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use user_core::user_id::UserId;

pub(crate) const MAX_JOB_BYTES: usize = 16 * 1024;
const SCHEMA_VERSION: u32 = 2;

#[derive(Deserialize)]
struct SchemaEnvelope {
    schema_version: u32,
}

#[derive(Serialize, Deserialize)]
struct EnvelopeV2 {
    schema_version: u32,
    scope: String,
    idempotency_key: String,
    ordering_key: String,
    #[serde(flatten)]
    job: PayloadV2,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "job_type", content = "payload")]
enum PayloadV2 {
    #[serde(rename = "PRODUCT_LISTING_EVENT")]
    ProductListingEvent {
        event_id: EventId,
        product_listing_id: ProductListingId,
    },
    #[serde(rename = "PRODUCT_LISTING_RAW_REVISION")]
    ProductListingRawRevision {
        product_listing_raw_stream_id: ProductListingRawStreamId,
        product_listing_raw_revision_id: ProductListingRawRevisionId,
        revision: u64,
    },
    #[serde(rename = "SEARCH_FILTER_CHANGED")]
    SearchFilterChanged {
        user_id: UserId,
        user_search_filter_id: UserSearchFilterId,
        version: i64,
        operation: OperationV2,
    },
    #[serde(rename = "SEARCH_FILTER_MATCH_CREATED")]
    SearchFilterMatchCreated {
        user_id: UserId,
        user_search_filter_id: UserSearchFilterId,
        product_listing_id: ProductListingId,
        origin_event_id: EventId,
    },
    #[serde(rename = "NOTIFICATION_DELIVERY_CREATED")]
    NotificationDeliveryCreated {
        notification_delivery_id: NotificationDeliveryId,
    },
}

#[derive(Serialize, Deserialize)]
enum OperationV2 {
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
        DomainJobPayload::ProductListingEvent(event) => PayloadV2::ProductListingEvent {
            event_id: event.event_id,
            product_listing_id: event.product_listing_id,
        },
        DomainJobPayload::ProductListingRawRevision(revision) => {
            PayloadV2::ProductListingRawRevision {
                product_listing_raw_stream_id: revision.product_listing_raw_stream_id,
                product_listing_raw_revision_id: revision.product_listing_raw_revision_id,
                revision: revision.revision,
            }
        }
        DomainJobPayload::SearchFilterChanged(change) => PayloadV2::SearchFilterChanged {
            user_id: change.user_id,
            user_search_filter_id: change.user_search_filter_id,
            version: change.version,
            operation: match change.operation {
                CdcOperation::Insert => OperationV2::Insert,
                CdcOperation::Update => OperationV2::Update,
                CdcOperation::Delete => OperationV2::Delete,
            },
        },
        DomainJobPayload::SearchFilterMatchCreated(change) => PayloadV2::SearchFilterMatchCreated {
            user_id: change.user_id,
            user_search_filter_id: change.user_search_filter_id,
            product_listing_id: change.product_listing_id,
            origin_event_id: change.origin_event_id,
        },
        DomainJobPayload::NotificationDeliveryCreated(delivery) => {
            PayloadV2::NotificationDeliveryCreated {
                notification_delivery_id: delivery.notification_delivery_id,
            }
        }
        DomainJobPayload::UserTierChanged(_) => return Err(WireError::Scope),
    };
    let encoded = serde_json::to_string(&EnvelopeV2 {
        schema_version: SCHEMA_VERSION,
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
    let schema: SchemaEnvelope = serde_json::from_str(body).map_err(|_| WireError::Json)?;
    if schema.schema_version != SCHEMA_VERSION {
        return Err(WireError::SchemaVersion);
    }
    let envelope: EnvelopeV2 = serde_json::from_str(body).map_err(|_| WireError::Json)?;
    let scope = WorkerScope::iter()
        .find(|scope| scope.as_str() == envelope.scope)
        .ok_or(WireError::Scope)?;
    if scope != expected_scope {
        return Err(WireError::Scope);
    }
    let payload = match envelope.job {
        PayloadV2::ProductListingEvent {
            event_id,
            product_listing_id,
        } => DomainJobPayload::ProductListingEvent(ProductListingEventJob {
            event_id,
            product_listing_id,
        }),
        PayloadV2::ProductListingRawRevision {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        } => DomainJobPayload::ProductListingRawRevision(ProductListingRawRevisionJob {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        }),
        PayloadV2::SearchFilterChanged {
            user_id,
            user_search_filter_id,
            version,
            operation,
        } => DomainJobPayload::SearchFilterChanged(SearchFilterChangedJob {
            user_id,
            user_search_filter_id,
            version,
            operation: match operation {
                OperationV2::Insert => CdcOperation::Insert,
                OperationV2::Update => CdcOperation::Update,
                OperationV2::Delete => CdcOperation::Delete,
            },
        }),
        PayloadV2::SearchFilterMatchCreated {
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
        PayloadV2::NotificationDeliveryCreated {
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

    const SUFFIX: &str = "01h455vb4pex5vy7enb1p677vn";
    const EVENT_ID: &str = "evt_01h455vb4pex5vy7enb1p677vn";
    const PRODUCT_LISTING_ID: &str = "pl_01h455vb4pex5vy7enb1p677vn";
    const RAW_STREAM_ID: &str = "prs_01h455vb4pex5vy7enb1p677vn";
    const RAW_REVISION_ID: &str = "prr_01h455vb4pex5vy7enb1p677vn";
    const USER_ID: &str = "usr_01h455vb4pex5vy7enb1p677vn";
    const SEARCH_FILTER_ID: &str = "sf_01h455vb4pex5vy7enb1p677vn";
    const NOTIFICATION_DELIVERY_ID: &str = "nd_01h455vb4pex5vy7enb1p677vn";
    const BARE_UUID_V7: &str = "01890a5d-ac96-774b-bf1d-d5586c639f75";

    fn snapshots() -> Vec<(WorkerScope, Value)> {
        let mut snapshots = vec![];
        for scope in WorkerScope::iter() {
            let (kind, payload, key, ordering) = match scope {
                WorkerScope::ProductListingRawNormalization => (
                    "PRODUCT_LISTING_RAW_REVISION",
                    json!({"product_listing_raw_stream_id": RAW_STREAM_ID, "product_listing_raw_revision_id": RAW_REVISION_ID, "revision": 3}),
                    format!("product-listing-raw-revision:{RAW_REVISION_ID}"),
                    format!("product-listing-raw-stream:{RAW_STREAM_ID}"),
                ),
                WorkerScope::SearchFilterProjection => (
                    "SEARCH_FILTER_CHANGED",
                    json!({"user_id": USER_ID, "user_search_filter_id": SEARCH_FILTER_ID, "version": 3, "operation": "UPDATE"}),
                    format!("search-filter:{SEARCH_FILTER_ID}:3:update"),
                    format!("search-filter:{SEARCH_FILTER_ID}"),
                ),
                WorkerScope::SearchFilterMatchNotification => (
                    "SEARCH_FILTER_MATCH_CREATED",
                    json!({"user_id": USER_ID, "user_search_filter_id": SEARCH_FILTER_ID, "product_listing_id": PRODUCT_LISTING_ID, "origin_event_id": EVENT_ID}),
                    format!(
                        "search-filter-match:{USER_ID}:{SEARCH_FILTER_ID}:{PRODUCT_LISTING_ID}:{EVENT_ID}"
                    ),
                    format!("user:{USER_ID}"),
                ),
                WorkerScope::NotificationDelivery => (
                    "NOTIFICATION_DELIVERY_CREATED",
                    json!({"notification_delivery_id": NOTIFICATION_DELIVERY_ID}),
                    format!("notification-delivery:{NOTIFICATION_DELIVERY_ID}"),
                    format!("notification-delivery:{NOTIFICATION_DELIVERY_ID}"),
                ),
                _ => (
                    "PRODUCT_LISTING_EVENT",
                    json!({"event_id": EVENT_ID, "product_listing_id": PRODUCT_LISTING_ID}),
                    format!("product-event:{EVENT_ID}"),
                    format!("product:{PRODUCT_LISTING_ID}"),
                ),
            };
            snapshots.push((
                scope,
                json!({"schema_version": 2, "scope": scope.as_str(), "job_type": kind,
                "idempotency_key": key, "ordering_key": ordering, "payload": payload}),
            ));
        }
        snapshots
    }

    #[test]
    fn should_match_v2_snapshots_for_every_scope_and_payload()
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
            let Some((_, mut snapshot)) = snapshots()
                .into_iter()
                .find(|(scope, _)| *scope == WorkerScope::SearchFilterProjection)
            else {
                return Err("missing search-filter projection snapshot".into());
            };
            snapshot["payload"]["operation"] = json!(wire);
            snapshot["idempotency_key"] =
                json!(format!("search-filter:{SEARCH_FILTER_ID}:3:{key}"));
            let job = decode(&snapshot.to_string(), WorkerScope::SearchFilterProjection)?;
            assert_eq!(snapshot, serde_json::from_str::<Value>(&encode(&job)?)?);
        }
        Ok(())
    }

    #[test]
    fn should_reject_schema_v1_without_compatibility_decoding() {
        let body = json!({
            "schema_version": 1,
            "scope": WorkerScope::ProductListingOpenSearch.as_str(),
            "job_type": "PRODUCT_LISTING_EVENT",
            "idempotency_key": format!("product-event:{BARE_UUID_V7}"),
            "ordering_key": format!("product:{BARE_UUID_V7}"),
            "payload": {
                "event_id": BARE_UUID_V7,
                "product_listing_id": BARE_UUID_V7
            }
        });

        assert_eq!(
            Err(WireError::SchemaVersion),
            decode(&body.to_string(), WorkerScope::ProductListingOpenSearch)
        );
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
                json!(BARE_UUID_V7),
                json!(format!("pty_{SUFFIX}")),
                json!(format!("EVT_{SUFFIX}")),
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
    fn should_reject_wrong_prefix_bare_uuid_malformed_and_noncanonical_typeids() {
        for (scope, original) in snapshots() {
            let Some(payload) = original["payload"].as_object() else {
                assert!(
                    original["payload"].is_object(),
                    "snapshot payload must be an object"
                );
                continue;
            };
            let id_fields: Vec<_> = payload
                .keys()
                .filter(|field| field.ends_with("id"))
                .cloned()
                .collect();
            for field in id_fields {
                for invalid in [
                    json!(format!("pty_{SUFFIX}")),
                    json!(BARE_UUID_V7),
                    json!("malformed"),
                    json!(format!("EVT_{SUFFIX}")),
                ] {
                    let mut body = original.clone();
                    body["payload"][&field] = invalid;
                    assert!(
                        decode(&body.to_string(), scope).is_err(),
                        "accepted invalid {field} for {}",
                        scope.as_str()
                    );
                }
            }
        }
    }

    #[test]
    fn should_reject_invalid_envelopes_and_payloads_for_every_variant() {
        for (scope, original) in snapshots() {
            for (field, value) in [
                ("schema_version", json!(0)),
                ("schema_version", json!(1)),
                ("schema_version", json!(3)),
                ("schema_version", json!("2")),
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

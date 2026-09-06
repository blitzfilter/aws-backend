# Admin overview

`GET /api/v1/admin/overview` is the administrator landing-page summary.

## Access and cache

- Requires a valid bearer credential and persisted `ADMIN` role for user or delegated-user callers.
- Service and system principals may call the use case internally.
- Returns `Cache-Control: no-store` on success and errors.
- Emits normal request/use-case structured logs only. It creates no audit record.

## Source and consistency

All values come from authoritative PostgreSQL, never OpenSearch or another rebuildable projection. The overview reader runs one aggregate SQL statement in the same PostgreSQL transaction that verifies the caller's persisted role. One statement gives every included aggregate one PostgreSQL MVCC statement snapshot.

| Response area | PostgreSQL source | Semantics |
| --- | --- | --- |
| `users` | `users` | Total users, then all canonical `tier` and `role` values. |
| `partnershipApplications` | `partnership_applications` | Total applications, then all canonical `business_state` values. |
| `parties` | `parties` | Total canonical Party rows. |
| `listingSources` | `listing_sources`, `listing_source_ingestion_methods` | Total sources; `withoutIngestionMethod` counts sources with no method rows. `methodAssignments` counts source-method rows, so its values can sum to more than the source total. |
| `partnerships` | `partnerships` | Total canonical Partnership rows. |
| `productListings` | `product_listings` | Total listings and lifecycle counts. Availability counts include only `ACTIVE` listings. `activeWithoutAvailability` counts active listings whose source has no current availability assertion; withdrawn listings always have null availability and are excluded. |

The response contains only fixed, bounded counters. `schemaVersion` is currently `1`; clients must use it to select compatible decoding when the contract grows.

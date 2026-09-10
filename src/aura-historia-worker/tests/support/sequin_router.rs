use axum::{
    Router, body::Bytes, extract::State, http::StatusCode, response::IntoResponse, routing::post,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

const MAX_BODY_BYTES: usize = 1024 * 1024;
const FORWARD_TIMEOUT: Duration = Duration::from_secs(12);

static ROUTER: OnceLock<Result<RouterControl, String>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct WalLsn(u64);

impl WalLsn {
    fn parse(value: &Value) -> Result<Self, String> {
        match value {
            Value::Number(value) => value
                .as_u64()
                .map(Self)
                .ok_or_else(|| "commit_lsn must be an unsigned integer".to_owned()),
            Value::String(value) => Self::parse_text(value),
            _ => Err("commit_lsn must be an integer or string".to_owned()),
        }
    }

    fn parse_text(value: &str) -> Result<Self, String> {
        if let Ok(value) = value.parse::<u64>() {
            return Ok(Self(value));
        }

        let Some((high, low)) = value.split_once('/') else {
            return Err(format!("invalid commit_lsn {value:?}"));
        };
        if low.contains('/') || high.is_empty() || low.is_empty() {
            return Err(format!("invalid commit_lsn {value:?}"));
        }
        let high = u32::from_str_radix(high, 16)
            .map_err(|error| format!("invalid commit_lsn high word {value:?}: {error}"))?;
        let low = u32::from_str_radix(low, 16)
            .map_err(|error| format!("invalid commit_lsn low word {value:?}: {error}"))?;
        Ok(Self((u64::from(high) << 32) | u64::from(low)))
    }
}

#[derive(Debug, Clone)]
struct ActiveRoute {
    generation: u64,
    fence_lsn: WalLsn,
    primary: SocketAddr,
    tables: BTreeMap<&'static str, SocketAddr>,
}

#[derive(Default)]
struct RouteState {
    active: Option<ActiveRoute>,
    fatal: Option<String>,
    next_generation: u64,
}

#[derive(Clone)]
struct RouterControl {
    state: Arc<Mutex<RouteState>>,
    client: reqwest::Client,
}

/// Owns a route generation. Dropping it makes the router inactive for that generation.
pub struct RouteLease {
    generation: u64,
    control: RouterControl,
}

impl Drop for RouteLease {
    fn drop(&mut self) {
        let mut state = match self.control.state.lock() {
            Ok(state) => state,
            Err(error) => error.into_inner(),
        };
        if state.active.as_ref().map(|route| route.generation) == Some(self.generation) {
            state.active = None;
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RouteTarget {
    pub table: &'static str,
    pub worker: SocketAddr,
}

#[derive(Debug, Clone)]
struct SequinMessage {
    table: String,
    commit_lsn: WalLsn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RouteDecision {
    InactiveDrop,
    StaleDrop,
    UnroutedDrop,
    Forward(SocketAddr),
}

pub async fn ensure_started() -> Result<(), String> {
    let router = ROUTER.get_or_init(start_router);
    router.as_ref().map(|_| ()).map_err(Clone::clone)
}

pub async fn activate_scope(
    scope: aura_historia_worker::WorkerScope,
    worker: SocketAddr,
) -> Result<RouteLease, String> {
    let table = match scope {
        aura_historia_worker::WorkerScope::ProductListingContentAssessment
        | aura_historia_worker::WorkerScope::ProductListingEmbedding
        | aura_historia_worker::WorkerScope::ProductListingTranslation
        | aura_historia_worker::WorkerScope::ProductListingOpenSearch
        | aura_historia_worker::WorkerScope::WatchlistNotification
        | aura_historia_worker::WorkerScope::SearchFilterPercolator => {
            "public.product_listing_events"
        }
        aura_historia_worker::WorkerScope::SearchFilterMatchNotification => {
            "public.search_filter_matches"
        }
        aura_historia_worker::WorkerScope::SearchFilterProjection => "public.search_filters",
        aura_historia_worker::WorkerScope::NotificationDelivery => "public.notification_deliveries",
        aura_historia_worker::WorkerScope::ProductListingRawNormalization => {
            "public.product_listing_raw_revisions"
        }
    };
    activate(worker, [RouteTarget { table, worker }]).await
}

pub async fn activate(
    primary: SocketAddr,
    targets: impl IntoIterator<Item = RouteTarget>,
) -> Result<RouteLease, String> {
    ensure_started().await?;
    let pool = test_api::get_postgres_client().await;
    let fence: String = sqlx::query_scalar("SELECT pg_current_wal_insert_lsn()::text")
        .fetch_one(&pool)
        .await
        .map_err(|error| format!("failed to capture worker route WAL fence: {error}"))?;
    let fence_lsn = WalLsn::parse_text(&fence)?;

    let mut tables = BTreeMap::new();
    for target in targets {
        if tables.insert(target.table, target.worker).is_some() {
            return Err(format!("duplicate worker route for table {}", target.table));
        }
    }
    if tables.is_empty() {
        return Err("worker route needs at least one table".to_owned());
    }

    install_route(router()?.clone(), fence_lsn, primary, tables)
}

fn install_route(
    control: RouterControl,
    fence_lsn: WalLsn,
    primary: SocketAddr,
    tables: BTreeMap<&'static str, SocketAddr>,
) -> Result<RouteLease, String> {
    let mut state = lock_state(&control);
    if let Some(active) = &state.active {
        return Err(format!(
            "cannot activate worker route while generation {} remains active for {:?}",
            active.generation,
            active.tables.keys().collect::<Vec<_>>()
        ));
    }
    state.next_generation = state.next_generation.saturating_add(1);
    let generation = state.next_generation;
    state.active = Some(ActiveRoute {
        generation,
        fence_lsn,
        primary,
        tables,
    });
    drop(state);
    Ok(RouteLease {
        generation,
        control,
    })
}

pub async fn post_direct_to_primary(change: Value) -> Result<(), String> {
    let control = router()?.clone();
    let primary = lock_state(&control)
        .active
        .as_ref()
        .map(|route| route.primary)
        .ok_or_else(|| "cannot post synthetic CDC without an active worker route".to_owned())?;
    let response = control
        .client
        .post(format!("http://{primary}/cdc/sequin"))
        .json(&change)
        .send()
        .await
        .map_err(|error| format!("synthetic CDC request to active worker failed: {error}"))?;
    if response.status() != StatusCode::ACCEPTED {
        return Err(format!(
            "active worker rejected synthetic CDC with status {}",
            response.status()
        ));
    }
    Ok(())
}

pub fn assert_idle() -> Result<(), String> {
    assert_control_idle(router()?)
}

fn assert_control_idle(control: &RouterControl) -> Result<(), String> {
    let state = lock_state(control);
    if let Some(fatal) = &state.fatal {
        return Err(format!(
            "Sequin test router fatal invariant violation: {fatal}"
        ));
    }
    if let Some(active) = &state.active {
        return Err(format!(
            "Sequin test router leaked generation {} for {:?}",
            active.generation,
            active.tables.keys().collect::<Vec<_>>()
        ));
    }
    Ok(())
}

fn router() -> Result<&'static RouterControl, String> {
    let router = ROUTER
        .get()
        .ok_or_else(|| "Sequin test router was not started".to_owned())?;
    router.as_ref().map_err(Clone::clone)
}

fn lock_state(control: &RouterControl) -> std::sync::MutexGuard<'_, RouteState> {
    match control.state.lock() {
        Ok(state) => state,
        Err(error) => error.into_inner(),
    }
}

fn start_router() -> Result<RouterControl, String> {
    let client = reqwest::Client::builder()
        .timeout(FORWARD_TIMEOUT)
        .build()
        .map_err(|error| format!("failed to build Sequin test router client: {error}"))?;
    let control = RouterControl {
        state: Arc::new(Mutex::new(RouteState::default())),
        client,
    };
    let server_control = control.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("worker-sequin-test-router".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready_tx.send(Err(format!(
                        "failed to build Sequin test router runtime: {error}"
                    )));
                    return;
                }
            };
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::bind(
                    test_api::get_sequin_worker_webhook_bind_addr(),
                )
                .await
                {
                    Ok(listener) => listener,
                    Err(error) => {
                        let _ = ready_tx
                            .send(Err(format!("failed to bind Sequin test router: {error}")));
                        return;
                    }
                };
                let app = Router::new()
                    .route("/cdc/sequin", post(receive))
                    .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
                    .with_state(server_control);
                let _ = ready_tx.send(Ok(()));
                if let Err(error) = axum::serve(listener, app).await {
                    panic!("Sequin test router stopped: {error}");
                }
            });
        })
        .map_err(|error| format!("failed to spawn Sequin test router thread: {error}"))?;
    ready_rx
        .recv()
        .map_err(|error| format!("failed to receive Sequin test router readiness: {error}"))??;
    Ok(control)
}

async fn receive(State(control): State<RouterControl>, body: Bytes) -> impl IntoResponse {
    let active = lock_state(&control).active.clone();
    let Some(active) = active else {
        return StatusCode::ACCEPTED;
    };

    let messages = match parse_messages(&body) {
        Ok(messages) => messages,
        Err(error) => return fatal(&control, error),
    };
    let destinations: Result<Vec<_>, _> = messages
        .iter()
        .map(|message| decide(Some(&active), message))
        .collect();
    let destinations = match destinations {
        Ok(destinations) => destinations,
        Err(error) => return fatal(&control, error),
    };

    let forward: Vec<_> = destinations
        .into_iter()
        .filter_map(|decision| match decision {
            RouteDecision::Forward(destination) => Some(destination),
            RouteDecision::InactiveDrop
            | RouteDecision::StaleDrop
            | RouteDecision::UnroutedDrop => None,
        })
        .collect();
    if forward.is_empty() {
        return StatusCode::ACCEPTED;
    }
    if forward.iter().any(|destination| *destination != forward[0]) {
        return fatal(
            &control,
            format!(
                "Sequin batch generation {} targets multiple scoped workers",
                active.generation
            ),
        );
    }
    if forward.len() != messages.len() {
        return fatal(
            &control,
            format!(
                "Sequin batch generation {} mixes current and dropped CDC messages",
                active.generation
            ),
        );
    }

    match control
        .client
        .post(format!("http://{}/cdc/sequin", forward[0]))
        .header("content-type", "application/json")
        .body(body.to_vec())
        .send()
        .await
    {
        Ok(response) => StatusCode::from_u16(response.status().as_u16())
            .unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        Err(error) => {
            tracing::debug!(
                generation = active.generation,
                destination = %forward[0],
                error = %error,
                "Sequin test router could not forward CDC"
            );
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

fn fatal(control: &RouterControl, error: String) -> StatusCode {
    lock_state(control).fatal = Some(error.clone());
    tracing::error!(error = %error, "Sequin test router rejected malformed CDC metadata");
    StatusCode::SERVICE_UNAVAILABLE
}

fn parse_messages(body: &[u8]) -> Result<Vec<SequinMessage>, String> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| format!("invalid Sequin webhook JSON: {error}"))?;
    let values = match value.get("data") {
        Some(Value::Array(values)) => values,
        Some(_) => return Err("Sequin webhook data must be an array".to_owned()),
        None => std::slice::from_ref(&value),
    };
    if values.is_empty() {
        return Err("Sequin webhook data must not be empty".to_owned());
    }
    values.iter().map(parse_message).collect()
}

fn parse_message(value: &Value) -> Result<SequinMessage, String> {
    let metadata = value
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or_else(|| "Sequin webhook metadata is missing or invalid".to_owned())?;
    let schema = metadata
        .get("table_schema")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Sequin webhook metadata.table_schema is missing or invalid".to_owned())?;
    let table = metadata
        .get("table_name")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Sequin webhook metadata.table_name is missing or invalid".to_owned())?;
    let commit_lsn = metadata
        .get("commit_lsn")
        .ok_or_else(|| "Sequin webhook metadata.commit_lsn is missing".to_owned())
        .and_then(WalLsn::parse)?;
    Ok(SequinMessage {
        table: format!("{schema}.{table}"),
        commit_lsn,
    })
}

fn decide(active: Option<&ActiveRoute>, message: &SequinMessage) -> Result<RouteDecision, String> {
    let Some(active) = active else {
        return Ok(RouteDecision::InactiveDrop);
    };
    if message.commit_lsn <= active.fence_lsn {
        return Ok(RouteDecision::StaleDrop);
    }
    Ok(active
        .tables
        .get(message.table.as_str())
        .copied()
        .map(RouteDecision::Forward)
        .unwrap_or(RouteDecision::UnroutedDrop))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(serde_json::json!(0), Ok(WalLsn(0)))]
    #[case(serde_json::json!(42), Ok(WalLsn(42)))]
    #[case(serde_json::json!("42"), Ok(WalLsn(42)))]
    #[case(serde_json::json!("16/B374D848"), Ok(WalLsn(0x16_B374D848)))]
    #[case(serde_json::json!(-1), Err(()))]
    #[case(serde_json::json!("-1"), Err(()))]
    #[case(serde_json::json!("100000000/0"), Err(()))]
    #[case(serde_json::json!("bad"), Err(()))]
    #[case(Value::Null, Err(()))]
    fn should_parse_supported_wal_lsns(#[case] input: Value, #[case] expected: Result<WalLsn, ()>) {
        assert_eq!(WalLsn::parse(&input).map_err(|_| ()), expected);
    }

    #[test]
    fn should_reject_missing_commit_lsn() {
        assert!(
            parse_messages(br#"{"metadata":{"table_schema":"public","table_name":"x"}}"#).is_err()
        );
    }

    #[test]
    fn should_choose_active_table_after_fence() {
        let first: SocketAddr = "127.0.0.1:10001".parse().unwrap();
        let second: SocketAddr = "127.0.0.1:10002".parse().unwrap();
        let active = ActiveRoute {
            generation: 1,
            fence_lsn: WalLsn(10),
            primary: first,
            tables: BTreeMap::from([("public.first", first), ("public.second", second)]),
        };
        let message = |table: &str, lsn: u64| SequinMessage {
            table: table.to_owned(),
            commit_lsn: WalLsn(lsn),
        };

        assert_eq!(
            decide(None, &message("public.first", 11)),
            Ok(RouteDecision::InactiveDrop)
        );
        assert_eq!(
            decide(Some(&active), &message("public.first", 9)),
            Ok(RouteDecision::StaleDrop)
        );
        assert_eq!(
            decide(Some(&active), &message("public.first", 10)),
            Ok(RouteDecision::StaleDrop)
        );
        assert_eq!(
            decide(Some(&active), &message("public.first", 11)),
            Ok(RouteDecision::Forward(first))
        );
        assert_eq!(
            decide(Some(&active), &message("public.second", 11)),
            Ok(RouteDecision::Forward(second))
        );
        assert_eq!(
            decide(Some(&active), &message("public.other", 11)),
            Ok(RouteDecision::UnroutedDrop)
        );
    }

    #[test]
    fn should_keep_new_generation_when_a_stale_lease_drops() -> Result<(), String> {
        let control = test_control();
        let worker: SocketAddr = "127.0.0.1:10001"
            .parse()
            .map_err(|error| format!("parse worker address: {error}"))?;
        let first = install_route(
            control.clone(),
            WalLsn(10),
            worker,
            BTreeMap::from([("public.first", worker)]),
        )?;
        assert!(
            install_route(
                control.clone(),
                WalLsn(11),
                worker,
                BTreeMap::from([("public.second", worker)]),
            )
            .is_err()
        );
        assert!(assert_control_idle(&control).is_err());
        drop(first);
        assert_control_idle(&control)?;

        let second = install_route(
            control.clone(),
            WalLsn(12),
            worker,
            BTreeMap::from([("public.second", worker)]),
        )?;
        let stale = RouteLease {
            generation: 1,
            control: control.clone(),
        };
        drop(stale);
        assert_eq!(
            lock_state(&control)
                .active
                .as_ref()
                .map(|route| route.generation),
            Some(2)
        );
        drop(second);
        assert_control_idle(&control)
    }

    #[test_api::serial]
    #[tokio::test]
    async fn should_forward_current_cdc_body_and_preserve_worker_status() -> Result<(), String> {
        let received = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/cdc/sequin", post(capture_body))
            .with_state(received.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("bind fake worker: {error}"))?;
        let worker = listener
            .local_addr()
            .map_err(|error| format!("read fake worker address: {error}"))?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let control = test_control_with_route(worker, WalLsn(10));
        let body = Bytes::from_static(
            br#"{"record":{"id":"current"},"action":"insert","metadata":{"table_schema":"public","table_name":"first","commit_lsn":11}}"#,
        );

        let response = receive(State(control), body.clone()).await.into_response();
        assert_eq!(StatusCode::ACCEPTED, response.status());
        assert_eq!(vec![body.to_vec()], *received.lock().await);
        server.abort();
        Ok(())
    }

    #[test_api::serial]
    #[tokio::test]
    async fn should_not_forward_delayed_same_table_cdc_across_route_generations()
    -> Result<(), String> {
        let received_a = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let app_a = Router::new()
            .route("/cdc/sequin", post(capture_body))
            .with_state(received_a.clone());
        let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("bind fake worker A: {error}"))?;
        let worker_a = listener_a
            .local_addr()
            .map_err(|error| format!("read fake worker A address: {error}"))?;
        let server_a = tokio::spawn(async move {
            let _ = axum::serve(listener_a, app_a).await;
        });

        let received_b = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let app_b = Router::new()
            .route("/cdc/sequin", post(capture_body))
            .with_state(received_b.clone());
        let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("bind fake worker B: {error}"))?;
        let worker_b = listener_b
            .local_addr()
            .map_err(|error| format!("read fake worker B address: {error}"))?;
        let server_b = tokio::spawn(async move {
            let _ = axum::serve(listener_b, app_b).await;
        });

        let control = test_control();
        let route_a = install_route(
            control.clone(),
            WalLsn(100),
            worker_a,
            BTreeMap::from([("public.product_listing_events", worker_a)]),
        )?;
        let a_current = Bytes::from_static(
            br#"{"record":{"id":"a-current"},"action":"insert","metadata":{"table_schema":"public","table_name":"product_listing_events","commit_lsn":101}}"#,
        );
        assert_eq!(
            StatusCode::ACCEPTED,
            receive(State(control.clone()), a_current.clone())
                .await
                .into_response()
                .status()
        );
        assert_eq!(vec![a_current.to_vec()], *received_a.lock().await);
        assert!(received_b.lock().await.is_empty());

        drop(route_a);
        let route_b = install_route(
            control.clone(),
            WalLsn(200),
            worker_b,
            BTreeMap::from([("public.product_listing_events", worker_b)]),
        )?;
        let a_delayed = Bytes::from_static(
            br#"{"record":{"id":"a-delayed"},"action":"insert","metadata":{"table_schema":"public","table_name":"product_listing_events","commit_lsn":150}}"#,
        );
        assert_eq!(
            StatusCode::ACCEPTED,
            receive(State(control.clone()), a_delayed.clone())
                .await
                .into_response()
                .status()
        );
        assert_eq!(vec![a_current.to_vec()], *received_a.lock().await);
        assert!(received_b.lock().await.is_empty());

        let b_current = Bytes::from_static(
            br#"{"record":{"id":"b-current"},"action":"insert","metadata":{"table_schema":"public","table_name":"product_listing_events","commit_lsn":201}}"#,
        );
        assert_eq!(
            StatusCode::ACCEPTED,
            receive(State(control.clone()), b_current.clone())
                .await
                .into_response()
                .status()
        );
        assert_eq!(vec![a_current.to_vec()], *received_a.lock().await);
        assert_eq!(vec![b_current.to_vec()], *received_b.lock().await);

        drop(route_b);
        server_a.abort();
        server_b.abort();
        assert!(matches!(server_a.await, Err(error) if error.is_cancelled()));
        assert!(matches!(server_b.await, Err(error) if error.is_cancelled()));
        assert_control_idle(&control)
    }

    #[test_api::serial]
    #[tokio::test]
    async fn should_not_forward_stale_or_wrong_table_cdc() -> Result<(), String> {
        let received = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/cdc/sequin", post(capture_body))
            .with_state(received.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("bind fake worker: {error}"))?;
        let worker = listener
            .local_addr()
            .map_err(|error| format!("read fake worker address: {error}"))?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let control = test_control_with_route(worker, WalLsn(10));
        for body in [
            Bytes::from_static(
                br#"{"metadata":{"table_schema":"public","table_name":"first","commit_lsn":10}}"#,
            ),
            Bytes::from_static(
                br#"{"metadata":{"table_schema":"public","table_name":"other","commit_lsn":11}}"#,
            ),
        ] {
            assert_eq!(
                StatusCode::ACCEPTED,
                receive(State(control.clone()), body)
                    .await
                    .into_response()
                    .status()
            );
        }
        assert!(received.lock().await.is_empty());
        server.abort();
        Ok(())
    }

    #[test_api::serial]
    #[tokio::test]
    async fn should_keep_backend_or_transport_failure_unacknowledged() -> Result<(), String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| format!("bind failing fake worker: {error}"))?;
        let worker = listener
            .local_addr()
            .map_err(|error| format!("read failing fake worker address: {error}"))?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                Router::new().route(
                    "/cdc/sequin",
                    post(|| async { StatusCode::SERVICE_UNAVAILABLE }),
                ),
            )
            .await;
        });
        let body = Bytes::from_static(
            br#"{"metadata":{"table_schema":"public","table_name":"first","commit_lsn":11}}"#,
        );
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE,
            receive(
                State(test_control_with_route(worker, WalLsn(10))),
                body.clone()
            )
            .await
            .into_response()
            .status()
        );
        server.abort();

        let unreachable: SocketAddr = "127.0.0.1:9"
            .parse()
            .map_err(|error| format!("parse unreachable address: {error}"))?;
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE,
            receive(
                State(test_control_with_route(unreachable, WalLsn(10))),
                body
            )
            .await
            .into_response()
            .status()
        );
        Ok(())
    }

    fn test_control() -> RouterControl {
        RouterControl {
            state: Arc::new(Mutex::new(RouteState::default())),
            client: reqwest::Client::builder()
                .timeout(FORWARD_TIMEOUT)
                .build()
                .unwrap_or_else(|error| panic!("build test router client: {error}")),
        }
    }

    fn test_control_with_route(worker: SocketAddr, fence_lsn: WalLsn) -> RouterControl {
        let control = test_control();
        *lock_state(&control) = RouteState {
            active: Some(ActiveRoute {
                generation: 1,
                fence_lsn,
                primary: worker,
                tables: BTreeMap::from([("public.first", worker)]),
            }),
            next_generation: 1,
            fatal: None,
        };
        control
    }

    async fn capture_body(
        State(received): State<Arc<tokio::sync::Mutex<Vec<Vec<u8>>>>>,
        body: Bytes,
    ) -> StatusCode {
        received.lock().await.push(body.to_vec());
        StatusCode::ACCEPTED
    }
}

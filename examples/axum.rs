//! Opt-in Axum + PostgreSQL durable polling example.
//!
//! Build/run only with `--features postgres-example`. The example never exposes a paid purchase
//! route: a consuming purchase coordinator must persist a decoded purchase before spawning work.

mod postgres_order_store;

use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use postgres_order_store::{
    record_successful_purchase_for_intent, OrderReference, OrderStore, StoredState,
};
use serde_json::json;
use smspool::{
    sms::{SmsCheck, SmsOrder},
    Client, Error,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct AppState {
    smspool: Client,
    store: Arc<OrderStore>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DATABASE_URL")?;
    let api_key = std::env::var("SMSPOOL_API_KEY")?;
    let order_key = std::env::var("SMSPOOL_ORDER_KEY")?;
    let owner = std::env::var("SMSPOOL_WORKER_OWNER").unwrap_or_else(|_| "axum-worker".to_owned());

    let smspool = Client::builder(api_key)
        .max_concurrency(8)
        .max_requests_per_second(32)
        .qps_wait_timeout(Duration::from_secs(5))
        .build()?;
    // A claim must outlive the longest provider call made while it is held, including retries
    // and Retry-After backoff. Deriving it from the client keeps this correct if a timeout is
    // retuned; the store rejects the pair outright if the margin is ever lost.
    let worst_case_in_flight = smspool.max_in_flight_duration();
    let lease = worst_case_in_flight
        .saturating_mul(2)
        .max(Duration::from_secs(60));
    let store = Arc::new(
        OrderStore::connect(
            &database_url,
            &order_key,
            owner,
            lease,
            worst_case_in_flight,
        )
        .await?,
    );
    store.migrate().await?;

    let state = AppState {
        smspool: smspool.clone(),
        store: store.clone(),
    };
    let shutdown = CancellationToken::new();
    let worker_state = state.clone();
    let worker_shutdown = shutdown.clone();
    let worker = tokio::spawn(async move { recovery_worker(worker_state, worker_shutdown).await });

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/orders/{reference}", get(order_status))
        .with_state(state);
    let bind = std::env::var("SMSPOOL_AXUM_BIND").unwrap_or_else(|_| "127.0.0.1:3000".to_owned());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    shutdown.cancel();
    let _ = worker.await;
    Ok(())
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

async fn order_status(
    State(state): State<AppState>,
    Path(reference): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let reference = OrderReference::parse(reference).map_err(AppError::store)?;
    let stored = state
        .store
        .status(&reference)
        .await
        .map_err(AppError::store)?;
    Ok(Json(json!({
        "reference": reference.as_str(),
        "state": stored,
    })))
}

async fn recovery_worker(state: AppState, shutdown: CancellationToken) {
    loop {
        if shutdown.is_cancelled() {
            break;
        }
        let mut claims = match state.store.claim_due(16).await {
            Ok(claims) => claims,
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        // One batched snapshot per cycle instead of one request per claim. The provider returns
        // `warning: "For high volume requests we recommend using /request/active instead of
        // /sms/check"`, and a claim batch of 16 would otherwise cost 16 requests per cycle.
        //
        // The snapshot is used *only* as a fast path for "definitely still pending". Every other
        // case still falls through to `sms/check`, which stays the authoritative source: a
        // refunded order simply vanishes from `request/active`, so absence alone cannot tell
        // "refunded" apart from "never listed". A failed snapshot degrades to the old
        // per-claim behaviour rather than mis-reporting anything as terminal.
        let snapshot = state.smspool.sms().active().await.ok();
        let still_pending: std::collections::BTreeSet<String> = snapshot
            .iter()
            .flatten()
            .filter(|order| !looks_delivered(order))
            .map(|order| order.order_code.as_str().to_owned())
            .collect();

        for claim in &mut claims {
            if shutdown.is_cancelled() {
                break;
            }
            let now = epoch_ms();
            if now >= claim.deadline_ms() {
                let _ = state.store.record_expired(claim).await;
                continue;
            }
            if still_pending.contains(claim.order_id().as_str()) {
                let _ = state.store.record_pending(claim, now + 5_000).await;
                continue;
            }
            let result = state.smspool.sms().check(claim.order_id()).await;
            let outcome = match result {
                Ok(SmsCheck::Pending(_)) => state.store.record_pending(claim, now + 5_000).await,
                Ok(SmsCheck::Received(_)) => {
                    state
                        .store
                        .record_terminal(claim, StoredState::Received)
                        .await
                }
                Ok(SmsCheck::Terminated(_)) => {
                    state
                        .store
                        .record_terminal(claim, StoredState::Terminated)
                        .await
                }
                Err(Error::OutcomeUnknown(_)) => {
                    state.store.record_reconcile_only(claim, now + 5_000).await
                }
                Err(_) => state.store.release_for_retry(claim, now + 5_000).await,
                Ok(_) => state.store.release_for_retry(claim, now + 5_000).await,
            };
            let _ = outcome;
        }
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    }
}

/// Integration point for a consuming purchase coordinator.
///
/// Call this synchronously after `purchase/sms` returns and before starting a worker. Before the
/// paid request, record an application intent with `OrderStore::record_purchase_intent`; if the
/// result is `OutcomeUnknown`, transition that intent with
/// `mark_purchase_intent_reconcile_only` and reconcile through the application's ledger. Once a
/// provider order is known, atomically persist it and resolve the intent with this function. This
/// example never retries a paid mutation automatically.
#[allow(dead_code)]
async fn persist_purchase_before_polling(
    store: &OrderStore,
    intent: &OrderReference,
    order: &SmsOrder,
) -> Result<OrderReference, postgres_order_store::StoreError> {
    let expires_ms = i64::try_from(order.expires_in)
        .unwrap_or(i64::MAX / 1_000)
        .saturating_mul(1_000);
    record_successful_purchase_for_intent(
        store,
        intent,
        order,
        epoch_ms().saturating_add(expires_ms),
    )
    .await
}

fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|value| i64::try_from(value.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

struct AppError(&'static str);

impl AppError {
    fn store(_: postgres_order_store::StoreError) -> Self {
        Self("storage failure")
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.0).into_response()
    }
}

/// Whether an active-snapshot row looks like it already carries a delivered SMS.
///
/// Deliberately one-sided: a false positive only costs one extra authoritative `sms/check`,
/// while a false negative would keep polling an order that is already done. It never decides a
/// terminal state on its own. Pending rows were observed carrying `code: "0"` and an empty
/// `full_code`, which is the sentinel this recognises.
fn looks_delivered(order: &smspool::sms::ActiveOrder) -> bool {
    let code = order.code.expose();
    order.full_code.is_some() || (!code.is_empty() && code != "0")
}

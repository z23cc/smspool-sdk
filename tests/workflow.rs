mod support;

use std::time::Duration;

use serde_json::{json, Value};
use smspool::{
    wait_for_code_with, wait_for_sms, ActiveOrdersWatcher, Client, OrderId, PollError, PollOptions,
    RetryPolicy, WatchError,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use support::{ResponseScript, Script, ScriptedServer};

fn client(server: &ScriptedServer) -> Client {
    Client::builder("workflow-test-key")
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .retry_policy(RetryPolicy::new(1).jitter_ratio(0.0))
        .build()
        .unwrap()
}

fn options(duration: Duration, interval: Duration) -> PollOptions {
    PollOptions::new(Instant::now() + duration, CancellationToken::new())
        .with_intervals(interval, interval)
        .unwrap()
        .with_jitter_ratio(0.0)
        .unwrap()
}

fn pending_sms() -> Value {
    json!({
        "expiration": 1_704_562_249_i64,
        "resend": 0,
        "status": 1,
        "time_left": 1173
    })
}

fn received_sms() -> Value {
    json!({
        "expiration": 1_704_562_249_i64,
        "full_sms": "Your verification code is 12345",
        "sms": "12345",
        "status": 3
    })
}

fn active_order(order_id: &str, status: &str, code: &str, full_code: &str) -> Value {
    json!({
        "code": code,
        "cost": "0.24",
        "expiry": 1_704_561_795_i64,
        "full_code": full_code,
        "order_code": order_id,
        "phonenumber": "1234567890",
        "service": "Test",
        "short_name": "US",
        "status": status,
        "time_left": 1126,
        "timestamp": "2024-01-06 18:03:15"
    })
}

#[tokio::test]
async fn cancellation_interrupts_an_in_flight_read_without_a_second_request() {
    let server = ScriptedServer::start([Script::Hang(Duration::from_secs(2))]).await;
    let client = client(&server);
    let cancellation = CancellationToken::new();
    let options = PollOptions::new(
        Instant::now() + Duration::from_secs(1),
        cancellation.clone(),
    )
    .with_intervals(Duration::from_millis(10), Duration::from_millis(10))
    .unwrap()
    .with_jitter_ratio(0.0)
    .unwrap();
    let order_id = OrderId::new("cancel-me").unwrap();

    let cancel_task = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancellation.cancel();
    });
    let result = wait_for_sms(&client, &order_id, options).await;
    cancel_task.await.unwrap();

    assert!(matches!(result, Err(PollError::Cancelled { .. })));
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn deadline_preserves_the_last_pending_snapshot() {
    let server =
        ScriptedServer::start([Script::Respond(ResponseScript::json(200, pending_sms()))]).await;
    let client = client(&server);
    let order_id = OrderId::new("deadline-order").unwrap();
    let result = wait_for_sms(
        &client,
        &order_id,
        options(Duration::from_millis(40), Duration::from_millis(100)),
    )
    .await;

    let error = result.unwrap_err();
    assert!(matches!(error, PollError::Deadline { .. }));
    assert!(error.last_observed().is_some());
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn caller_supplied_extractor_receives_full_sms_text() {
    let server =
        ScriptedServer::start([Script::Respond(ResponseScript::json(200, received_sms()))]).await;
    let client = client(&server);
    let order_id = OrderId::new("extract-order").unwrap();

    let result = wait_for_code_with(
        &client,
        &order_id,
        options(Duration::from_secs(1), Duration::from_millis(5)),
        |text| text.split_whitespace().last().map(str::to_owned),
    )
    .await
    .unwrap();

    assert_eq!(result.code.as_deref(), Some("12345"));
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn exhausted_rate_limit_retry_after_is_a_polling_delay_floor() {
    let server = ScriptedServer::start([
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0, "message": "slow down"}))
                .header("retry-after", "1"),
        ),
        Script::Respond(ResponseScript::json(200, received_sms())),
    ])
    .await;
    let client = client(&server);
    let order_id = OrderId::new("rate-limited-order").unwrap();
    let started = Instant::now();

    let result = wait_for_sms(
        &client,
        &order_id,
        options(Duration::from_secs(2), Duration::from_millis(5)),
    )
    .await;

    assert!(result.is_ok());
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(server.request_count(), 2);
}

#[tokio::test]
async fn watcher_suppresses_duplicates_and_emits_status_or_code_changes() {
    let initial = active_order("watch-1", "pending", "0", "");
    let changed = active_order("watch-1", "complete", "12345", "code 12345");
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::json(200, json!([initial.clone()]))),
        Script::Respond(ResponseScript::json(200, json!([initial]))),
        Script::Respond(ResponseScript::json(200, json!([changed]))),
    ])
    .await;
    let mut watcher = ActiveOrdersWatcher::new(
        client(&server),
        options(Duration::from_secs(1), Duration::from_millis(5)),
        4,
    )
    .unwrap();

    let first = watcher.next().await.unwrap();
    let second = watcher.next().await.unwrap();

    assert_eq!(first.order_code.as_str(), "watch-1");
    assert_eq!(second.code.expose(), "12345");
    assert_eq!(second.status, "complete");
    assert_eq!(server.request_count(), 3);
    assert_eq!(watcher.tracked_order_count(), 1);
}

#[tokio::test]
async fn watcher_forgets_orders_absent_from_the_active_snapshot() {
    let order = active_order("watch-reappear", "pending", "0", "");
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::json(200, json!([order.clone()]))),
        Script::Respond(ResponseScript::json(200, json!([]))),
        Script::Respond(ResponseScript::json(200, json!([order]))),
    ])
    .await;
    let mut watcher = ActiveOrdersWatcher::new(
        client(&server),
        options(Duration::from_secs(1), Duration::from_millis(5)),
        4,
    )
    .unwrap();

    watcher.next().await.unwrap();
    let reappeared = watcher.next().await.unwrap();

    assert_eq!(reappeared.order_code.as_str(), "watch-reappear");
    assert_eq!(server.request_count(), 3);
}

#[tokio::test]
async fn watcher_cancellation_preempts_an_already_queued_event() {
    let server = ScriptedServer::start([Script::Respond(ResponseScript::json(
        200,
        json!([
            active_order("queued-1", "pending", "0", ""),
            active_order("queued-2", "pending", "0", "")
        ]),
    ))])
    .await;
    let cancellation = CancellationToken::new();
    let poll_options = PollOptions::new(
        Instant::now() + Duration::from_secs(1),
        cancellation.clone(),
    )
    .with_intervals(Duration::from_millis(5), Duration::from_millis(5))
    .unwrap()
    .with_jitter_ratio(0.0)
    .unwrap();
    let mut watcher = ActiveOrdersWatcher::new(client(&server), poll_options, 4).unwrap();

    watcher.next().await.unwrap();
    cancellation.cancel();
    let error = watcher.next().await.unwrap_err();

    assert!(matches!(error, WatchError::Cancelled { .. }));
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn watcher_deadline_preempts_an_already_queued_event() {
    let server = ScriptedServer::start([Script::Respond(ResponseScript::json(
        200,
        json!([
            active_order("queued-1", "pending", "0", ""),
            active_order("queued-2", "pending", "0", "")
        ]),
    ))])
    .await;
    let poll_options = PollOptions::new(
        Instant::now() + Duration::from_millis(30),
        CancellationToken::new(),
    )
    .with_intervals(Duration::from_millis(5), Duration::from_millis(5))
    .unwrap()
    .with_jitter_ratio(0.0)
    .unwrap();
    let mut watcher = ActiveOrdersWatcher::new(client(&server), poll_options, 4).unwrap();

    watcher.next().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let error = watcher.next().await.unwrap_err();

    assert!(matches!(error, WatchError::Deadline { .. }));
    assert_eq!(server.request_count(), 1);
}

#[tokio::test]
async fn watcher_fails_locally_before_growing_beyond_its_bound() {
    let server = ScriptedServer::start([Script::Respond(ResponseScript::json(
        200,
        json!([
            active_order("watch-1", "pending", "0", ""),
            active_order("watch-2", "pending", "0", "")
        ]),
    ))])
    .await;
    let mut watcher = ActiveOrdersWatcher::new(
        client(&server),
        options(Duration::from_secs(1), Duration::from_millis(5)),
        1,
    )
    .unwrap();

    let error = watcher.next().await.unwrap_err();
    assert!(matches!(
        error,
        WatchError::TrackingLimitExceeded {
            limit: 1,
            observed: 2
        }
    ));
    assert_eq!(watcher.tracked_order_count(), 0);
    assert_eq!(server.request_count(), 1);
}

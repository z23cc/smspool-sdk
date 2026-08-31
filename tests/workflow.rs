mod support;

use std::{str::FromStr, time::Duration};

use http::StatusCode;
use rust_decimal::Decimal;
use serde_json::{json, Value};
use smspool::{
    cancel_with_reconciliation, wait_for_code_with, wait_for_sms, ActiveOrdersWatcher,
    BalanceObservation, CancelOptions, CancelTimeLockRule, CancellationDisposition,
    CheckObservation, Client, ExpectedRefundMatch, Money, OrderId, PollError, PollOptions,
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
async fn cancellation_retries_only_an_exact_time_lock_and_records_refund_delta() {
    let exact = "Your order cannot be cancelled yet, please try again later.";
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::json(200, json!({"balance": "1.00"}))),
        Script::Respond(ResponseScript::json(
            400,
            json!({"success": 0, "message": exact}),
        )),
        Script::Respond(ResponseScript::json(200, pending_sms())),
        Script::Respond(ResponseScript::json(200, json!([]))),
        Script::Respond(ResponseScript::json(200, json!({"balance": "1.00"}))),
        Script::Respond(ResponseScript::json(
            200,
            json!({"success": 1, "message": "cancelled"}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 6, "message": "refunded"}),
        )),
        Script::Respond(ResponseScript::json(200, json!([]))),
        Script::Respond(ResponseScript::json(200, json!({"balance": "1.02"}))),
    ])
    .await;
    let rule =
        CancelTimeLockRule::message(StatusCode::BAD_REQUEST, exact, Duration::from_millis(1))
            .unwrap();
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("cancel-flow").unwrap(),
        CancelOptions::new(options(Duration::from_secs(1), Duration::from_millis(1)))
            .max_cancel_attempts(2)
            .max_outcome_unknown_reconciliation_checks(1)
            .time_lock_rule(rule)
            .expected_refund(Money::from_str("0.02").unwrap()),
    )
    .await
    .unwrap();

    assert!(matches!(
        result.disposition,
        CancellationDisposition::CancellationAccepted
    ));
    assert_eq!(result.cancel_attempts, 2);
    assert_eq!(result.reconciliation_checks, 2);
    assert_eq!(result.check, CheckObservation::Terminated);
    assert_eq!(
        result.balance,
        BalanceObservation::Delta {
            amount: smspool::SignedMoneyDelta::new(Decimal::from_str("0.02").unwrap()),
            expected_refund_match: ExpectedRefundMatch::Matches,
        }
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        2
    );
}

/// `SmsCheck` infers `Terminated` from the presence of a `message` alone, and this vendor also
/// returns prose on non-terminal responses. If such a message appears while the order is still
/// listed by `request/active`, abandoning cancellation would leave a live, unrefunded number.
#[tokio::test]
async fn cancellation_continues_when_active_contradicts_a_message_only_terminal_check() {
    let exact = "Your order cannot be cancelled yet, please try again later.";
    let order_id = "contradicted-order";
    let server = ScriptedServer::start([
        // Attempt 1: time-locked. Reconciliation reports a message-only "terminal" check while
        // request/active still lists the order, so the two observations disagree.
        Script::Respond(ResponseScript::json(
            400,
            json!({"success": 0, "message": exact}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 1, "message": "Your number is still being processed."}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!([active_order(order_id, "pending", "0", "")]),
        )),
        // Attempt 2 must still happen, and here it succeeds.
        Script::Respond(ResponseScript::json(
            200,
            json!({"success": 1, "message": "cancelled"}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 6, "message": "refunded"}),
        )),
        Script::Respond(ResponseScript::json(200, json!([]))),
    ])
    .await;
    let rule =
        CancelTimeLockRule::message(StatusCode::BAD_REQUEST, exact, Duration::from_millis(1))
            .unwrap();
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new(order_id).unwrap(),
        CancelOptions::new(options(Duration::from_secs(1), Duration::from_millis(1)))
            .max_cancel_attempts(2)
            .max_outcome_unknown_reconciliation_checks(1)
            .time_lock_rule(rule),
    )
    .await
    .unwrap();

    assert!(
        matches!(
            result.disposition,
            CancellationDisposition::CancellationAccepted
        ),
        "expected the cancellation to proceed, got {:?} check={:?} active={:?}",
        result.disposition,
        result.check,
        result.active
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        2,
        "a contradicted terminal check must not abandon cancellation"
    );
}

/// An undecodable `request/active` snapshot is *not* agreement that the order is settled.
/// Treating `Unavailable` as corroboration would reintroduce the abandoned-cancellation path
/// precisely when the vendor payload drifts, which is when it is most likely to happen.
#[tokio::test]
async fn cancellation_continues_when_active_is_undecodable() {
    let exact = "Your order cannot be cancelled yet, please try again later.";
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::json(
            400,
            json!({"success": 0, "message": exact}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 1, "message": "Your number is still being processed."}),
        )),
        // Shape drift: `code` is absent, so the active snapshot cannot be decoded.
        Script::Respond(ResponseScript::json(
            200,
            json!([{"order_code": "drifted-order", "unexpected": true}]),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"success": 1, "message": "cancelled"}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 6, "message": "refunded"}),
        )),
        Script::Respond(ResponseScript::json(200, json!([]))),
    ])
    .await;
    let rule =
        CancelTimeLockRule::message(StatusCode::BAD_REQUEST, exact, Duration::from_millis(1))
            .unwrap();
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("drifted-order").unwrap(),
        CancelOptions::new(options(Duration::from_secs(1), Duration::from_millis(1)))
            .max_cancel_attempts(2)
            .max_outcome_unknown_reconciliation_checks(1)
            .time_lock_rule(rule),
    )
    .await
    .unwrap();

    assert_eq!(result.check, CheckObservation::Terminated);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        2,
        "an unavailable active snapshot must not settle the order"
    );
    assert!(matches!(
        result.disposition,
        CancellationDisposition::CancellationAccepted
    ));
}

/// The cost of treating `Unavailable` as "keep going": if the order really was settled, one
/// extra `sms/cancel` is sent and rejected. That must resolve to `Inconclusive` rather than
/// looping, so a persistence layer has a distinct signal from `StillActive`.
#[tokio::test]
async fn undecodable_active_on_a_settled_order_resolves_inconclusive() {
    let exact = "Your order cannot be cancelled yet, please try again later.";
    let drifted = json!([{"order_code": "settled-drifted", "unexpected": true}]);
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::json(
            400,
            json!({"success": 0, "message": exact}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 6, "message": "This order has been refunded"}),
        )),
        Script::Respond(ResponseScript::json(200, drifted.clone())),
        // The order is already gone, so the second cancel is rejected with a non-time-lock error.
        Script::Respond(ResponseScript::json(
            404,
            json!({"success": 0, "message": "We could not find this order."}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 6, "message": "This order has been refunded"}),
        )),
        Script::Respond(ResponseScript::json(200, drifted)),
    ])
    .await;
    let rule =
        CancelTimeLockRule::message(StatusCode::BAD_REQUEST, exact, Duration::from_millis(1))
            .unwrap();
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("settled-drifted").unwrap(),
        CancelOptions::new(options(Duration::from_secs(1), Duration::from_millis(1)))
            .max_cancel_attempts(5)
            .max_outcome_unknown_reconciliation_checks(1)
            .time_lock_rule(rule),
    )
    .await
    .unwrap();

    assert_eq!(result.check, CheckObservation::Terminated);
    assert_eq!(result.active, smspool::ActiveObservation::Unavailable);
    assert!(
        matches!(result.disposition, CancellationDisposition::Inconclusive),
        "an unconfirmable settled order must be inconclusive, got {:?}",
        result.disposition
    );
    // Bounded: it stops at the rejection instead of running to max_cancel_attempts.
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        2
    );
}

/// A 429 on a cancel attempt means "not executed, retry later" — it must not end the workflow
/// while cancel budget and deadline both remain, or the number stays live and unrefunded.
/// `sms/cancel` is a Mutation, so the transport never retries it; the workflow must.
#[tokio::test]
async fn cancellation_survives_a_rate_limited_attempt() {
    let exact = "Your order cannot be cancelled yet, please try again later.";
    let server = ScriptedServer::start([
        // Attempt 1 is rate limited before the provider acts on it.
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0, "message": "slow down"}))
                .header("retry-after", "0"),
        ),
        Script::Respond(ResponseScript::json(200, pending_sms())),
        Script::Respond(ResponseScript::json(200, json!([]))),
        // Attempt 2 must still be made.
        Script::Respond(ResponseScript::json(
            200,
            json!({"success": 1, "message": "cancelled"}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 6, "message": "refunded"}),
        )),
        Script::Respond(ResponseScript::json(200, json!([]))),
    ])
    .await;
    let rule =
        CancelTimeLockRule::message(StatusCode::BAD_REQUEST, exact, Duration::from_millis(1))
            .unwrap();
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("rate-limited-cancel").unwrap(),
        CancelOptions::new(options(Duration::from_secs(5), Duration::from_millis(1)))
            .max_cancel_attempts(5)
            .max_outcome_unknown_reconciliation_checks(1)
            .time_lock_rule(rule),
    )
    .await
    .unwrap();

    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        2,
        "a rate-limited cancel must be retried within the remaining budget"
    );
    assert!(
        matches!(
            result.disposition,
            CancellationDisposition::CancellationAccepted
        ),
        "got {:?}",
        result.disposition
    );
}

/// Repeated rate limiting must terminate at the cancel budget *and* actually back off.
///
/// `Retry-After: 0` must not produce a tight loop against a paid endpoint: `wait_cancel_retry`
/// floors the delay at `base_interval`, and this pins that floor against future refactors.
#[tokio::test]
async fn persistent_rate_limiting_stops_at_the_cancel_budget() {
    let server = ScriptedServer::start([
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0, "message": "slow down"}))
                .header("retry-after", "0"),
        ),
        Script::Respond(ResponseScript::json(200, pending_sms())),
        Script::Respond(ResponseScript::json(200, json!([]))),
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0, "message": "slow down"}))
                .header("retry-after", "0"),
        ),
        Script::Respond(ResponseScript::json(200, pending_sms())),
        Script::Respond(ResponseScript::json(200, json!([]))),
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0, "message": "slow down"}))
                .header("retry-after", "0"),
        ),
        Script::Respond(ResponseScript::json(200, pending_sms())),
        Script::Respond(ResponseScript::json(200, json!([]))),
    ])
    .await;
    let started = std::time::Instant::now();
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("always-limited").unwrap(),
        CancelOptions::new(options(Duration::from_secs(5), Duration::from_millis(80)))
            .max_cancel_attempts(3)
            .max_outcome_unknown_reconciliation_checks(1),
    )
    .await
    .unwrap();
    let elapsed = started.elapsed();

    // Two waits between three attempts, floored at base_interval despite `Retry-After: 0`.
    assert!(
        elapsed >= Duration::from_millis(120),
        "rate-limited retries must back off, took only {elapsed:?}"
    );
    assert_eq!(result.cancel_attempts, 3);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        3,
        "must not exceed the cancel budget"
    );
    assert!(
        matches!(result.disposition, CancellationDisposition::StillActive),
        "a still-pending order must report StillActive, got {:?}",
        result.disposition
    );
}

/// The corroborated case must still short-circuit: a terminal check with the order absent from
/// request/active means the order really is settled, so no further cancel is sent.
#[tokio::test]
async fn cancellation_stops_when_active_corroborates_a_terminal_check() {
    let exact = "Your order cannot be cancelled yet, please try again later.";
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::json(
            400,
            json!({"success": 0, "message": exact}),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"status": 6, "message": "This order has been refunded"}),
        )),
        Script::Respond(ResponseScript::json(200, json!([]))),
    ])
    .await;
    let rule =
        CancelTimeLockRule::message(StatusCode::BAD_REQUEST, exact, Duration::from_millis(1))
            .unwrap();
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("settled-order").unwrap(),
        CancelOptions::new(options(Duration::from_secs(1), Duration::from_millis(1)))
            .max_cancel_attempts(5)
            .max_outcome_unknown_reconciliation_checks(1)
            .time_lock_rule(rule),
    )
    .await
    .unwrap();

    assert!(matches!(
        result.disposition,
        CancellationDisposition::TerminalSms
    ));
    assert_eq!(result.check, CheckObservation::Terminated);
    assert_eq!(result.active, smspool::ActiveObservation::Absent);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        1,
        "a corroborated terminal check must not keep cancelling"
    );
}

#[tokio::test]
async fn cancellation_does_not_retry_a_non_matching_error() {
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::json(
            400,
            json!({"success": 0, "message": "please wait"}),
        )),
        Script::Respond(ResponseScript::json(200, pending_sms())),
        Script::Respond(ResponseScript::json(200, json!([]))),
    ])
    .await;
    let rule = CancelTimeLockRule::message(
        StatusCode::BAD_REQUEST,
        "exact different message",
        Duration::from_millis(1),
    )
    .unwrap();
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("no-retry").unwrap(),
        CancelOptions::new(options(Duration::from_secs(1), Duration::from_millis(1)))
            .max_cancel_attempts(3)
            .time_lock_rule(rule),
    )
    .await
    .unwrap();
    assert!(matches!(
        result.disposition,
        CancellationDisposition::Inconclusive
    ));
    assert_eq!(result.cancel_attempts, 1);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        1
    );
}

#[tokio::test]
async fn cancellation_outcome_unknown_reconciles_without_replaying_mutation() {
    let server = ScriptedServer::start([
        Script::Disconnect,
        Script::Respond(ResponseScript::json(200, pending_sms())),
        Script::Respond(ResponseScript::json(200, json!([]))),
    ])
    .await;
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("unknown-cancel").unwrap(),
        CancelOptions::new(options(Duration::from_secs(1), Duration::from_millis(1)))
            .max_outcome_unknown_reconciliation_checks(1),
    )
    .await
    .unwrap();
    assert!(matches!(
        result.disposition,
        CancellationDisposition::OutcomeUnknown(_)
    ));
    assert_eq!(result.cancel_attempts, 1);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.target == "/sms/cancel")
            .count(),
        1
    );
}

#[tokio::test]
async fn cancellation_preflight_happens_before_balance_observation() {
    let server = ScriptedServer::start([]).await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let poll = PollOptions::new(Instant::now() + Duration::from_secs(1), cancellation);
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("preflight-cancel").unwrap(),
        CancelOptions::new(poll).observe_balance(true),
    )
    .await;
    assert!(matches!(
        result,
        Err(smspool::CancelWorkflowError::Cancelled)
    ));
    assert_eq!(server.request_count(), 0);
}

#[tokio::test]
async fn cancellation_rule_and_limits_validate_without_network() {
    assert!(CancelTimeLockRule::message(StatusCode::OK, "x", Duration::from_secs(1)).is_err());
    assert!(
        CancelTimeLockRule::message(StatusCode::BAD_REQUEST, "", Duration::from_secs(1)).is_err()
    );
    assert!(CancelTimeLockRule::message(StatusCode::BAD_REQUEST, "x", Duration::ZERO).is_err());
    let server = ScriptedServer::start([]).await;
    let result = cancel_with_reconciliation(
        &client(&server),
        &OrderId::new("invalid-cancel").unwrap(),
        CancelOptions::new(options(Duration::from_secs(1), Duration::from_millis(1)))
            .max_cancel_attempts(0),
    )
    .await;
    assert!(matches!(
        result,
        Err(smspool::CancelWorkflowError::InvalidOptions(_))
    ));
    assert_eq!(server.request_count(), 0);
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

/// Synthetic coverage for numeric `sms/check` statuses.
///
/// Only 1/3/6 have Postman evidence; 2/4/5/7/8 have none. This pins exactly what the SDK does
/// with each shape so a deserializer change cannot silently reclassify an order, and documents
/// that classification is shape-driven while the raw status stays reachable.
#[tokio::test]
async fn numeric_status_classification_is_pinned_for_every_known_and_unknown_code() {
    use smspool::{types::StatusValue, CheckObservation};

    // (status, extra fields, expected variant, expected terminal)
    let cases: Vec<(i64, Value, &str, bool)> = vec![
        // Evidenced by Postman fixtures.
        (1, json!({"time_left": 100}), "pending", false),
        (
            3,
            json!({"sms": "12345", "full_sms": "code 12345"}),
            "received",
            true,
        ),
        (
            6,
            json!({"message": "This order has been refunded"}),
            "terminated",
            true,
        ),
        // No vendor evidence. Bare numeric status must stay pending, never guessed terminal.
        (2, json!({}), "pending", false),
        (4, json!({}), "pending", false),
        (5, json!({}), "pending", false),
        (7, json!({}), "pending", false),
        (8, json!({}), "pending", false),
        // Same unknown codes carrying prose are treated as terminated by shape alone.
        (
            4,
            json!({"message": "something happened"}),
            "terminated",
            true,
        ),
        (
            7,
            json!({"message": "something happened"}),
            "terminated",
            true,
        ),
        // SMS content always wins over a co-present message.
        (
            8,
            json!({"sms": "99999", "message": "delivered"}),
            "received",
            true,
        ),
    ];

    for (status, extra, expected, terminal) in cases {
        let mut body = json!({"status": status});
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        let server =
            ScriptedServer::start([Script::Respond(ResponseScript::json(200, body))]).await;
        let check = client(&server)
            .sms()
            .check(&OrderId::new("status-probe").unwrap())
            .await
            .unwrap_or_else(|error| panic!("status {status} must decode, got {error:?}"));

        let actual = match &check {
            smspool::sms::SmsCheck::Pending(_) => "pending",
            smspool::sms::SmsCheck::Received(_) => "received",
            smspool::sms::SmsCheck::Terminated(_) => "terminated",
            _ => "unknown",
        };
        assert_eq!(actual, expected, "status {status} classified as {actual}");
        assert_eq!(
            check.is_terminal(),
            terminal,
            "status {status} terminal mismatch"
        );
        // The raw status is preserved verbatim regardless of the variant chosen.
        assert_eq!(check.status_code(), Some(status), "status {status} lost");
        assert_eq!(*check.status(), StatusValue::Integer(status));
    }

    // A non-numeric status must not panic or become a code.
    let server = ScriptedServer::start([Script::Respond(ResponseScript::json(
        200,
        json!({"status": "waiting"}),
    ))])
    .await;
    let check = client(&server)
        .sms()
        .check(&OrderId::new("status-probe").unwrap())
        .await
        .unwrap();
    assert_eq!(check.status_code(), None);
    assert!(!check.is_terminal());
    let _ = CheckObservation::Pending;
}

/// Shape confirmed live against two real completed orders: `sms` is the extracted code and
/// `full_sms` is the whole message *containing* it, and real messages are multi-byte UTF-8
/// (35 chars / 71 bytes in one observed sample). Every Postman fixture is ASCII, so this pins
/// the non-ASCII path and the redaction guarantee against real-world content.
#[tokio::test]
async fn received_sms_decodes_multibyte_content_without_leaking_it() {
    // Synthetic, but structurally identical to a real observed message: a 6-digit code that
    // also appears inside a longer multi-byte body. Never put real customer content in fixtures.
    let code = "482915";
    let body = "【示例服务】您的验证码是 482915，请勿泄露给他人。";
    assert!(
        body.len() > body.chars().count(),
        "sample must be multi-byte"
    );

    let server = ScriptedServer::start([Script::Respond(ResponseScript::json(
        200,
        json!({"status": 3, "sms": code, "full_sms": body, "expiration": 1_704_562_249_i64}),
    ))])
    .await;
    let check = client(&server)
        .sms()
        .check(&OrderId::new("utf8-order").unwrap())
        .await
        .unwrap();

    assert!(check.is_terminal());
    assert_eq!(check.status_code(), Some(3));
    let smspool::sms::SmsCheck::Received(received) = &check else {
        panic!("expected Received, got {check:?}");
    };
    let short = received.sms.as_ref().expect("sms").expose();
    let full = received.full_sms.as_ref().expect("full_sms").expose();
    assert_eq!(short, code);
    assert_eq!(full, body);
    assert_ne!(short, full, "the two fields carry different content");
    assert!(full.contains(short), "full_sms must contain the short code");
    assert!(!full.is_ascii(), "multi-byte content must survive decoding");

    // Redaction must hold for real customer content, not just ASCII placeholders.
    for rendered in [
        format!("{:?}", received.sms),
        format!("{:?}", received.full_sms),
        format!("{check:?}"),
    ] {
        assert!(!rendered.contains(code), "code leaked into {rendered}");
        assert!(!rendered.contains(body), "message leaked into {rendered}");
    }
}

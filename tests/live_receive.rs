//! Opt-in live capture of the SMS *receive* path.
//!
//! Every automated test and every live run so far has cancelled while pending, so
//! [`SmsCheck::Received`], `SmsText` decoding, the `full_sms` vs `sms` split, and
//! `wait_for_sms` terminal detection have only fixture coverage. Closing that gap needs a real
//! inbound message, which needs a real service the operator is legitimately signing up for.
//!
//! This harness exists so that capture is one command when that moment arrives. It is `#[ignore]`d
//! and additionally requires explicit opt-in, because it spends money that is **not refundable
//! once an SMS lands**.
//!
//! ```text
//! SMSPOOL_API_KEY=...            \
//! SMSPOOL_LIVE_RECEIVE=i-accept-non-refundable-cost \
//! SMSPOOL_LIVE_COUNTRY=1         \
//! SMSPOOL_LIVE_SERVICE=395       \
//! SMSPOOL_LIVE_MAX_PRICE=0.50    \
//! cargo test --test live_receive -- --ignored --nocapture
//! ```
//!
//! It prints the purchased number (the operator needs it) and structural facts about the reply.
//! The SMS body and code are printed only under `SMSPOOL_LIVE_PRINT_CODE=1`.

use std::{str::FromStr, time::Duration};

use http::StatusCode;
use smspool::{
    sms::{PurchaseSmsRequest, PurchaseSmsResponse, SmsCheck},
    wait_for_sms, CancelOptions, CancelTimeLockRule, Client, CountryId, Money, PollOptions,
    ServiceId,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required for the live receive test"))
}

#[tokio::test]
#[ignore = "spends non-refundable money and needs a real inbound SMS"]
async fn live_receive_captures_the_sms_shape() {
    assert_eq!(
        std::env::var("SMSPOOL_LIVE_RECEIVE").as_deref(),
        Ok("i-accept-non-refundable-cost"),
        "explicit opt-in is required"
    );
    let cap = Money::from_str(&required("SMSPOOL_LIVE_MAX_PRICE")).expect("max price");
    let country = CountryId::new(required("SMSPOOL_LIVE_COUNTRY")).unwrap();
    let service = ServiceId::new(required("SMSPOOL_LIVE_SERVICE")).unwrap();
    let print_code = std::env::var("SMSPOOL_LIVE_PRINT_CODE").as_deref() == Ok("1");

    let client = Client::builder(required("SMSPOOL_API_KEY"))
        .max_requests_per_second(4)
        .build()
        .expect("client");

    let order = match client
        .sms()
        .purchase(&PurchaseSmsRequest::new(country, service).max_price(cap))
        .await
        .expect("purchase")
    {
        PurchaseSmsResponse::Order(order) => order,
        other => panic!("unexpected purchase response: {other:?}"),
    };
    println!(
        "purchased cost={} expires_in={}s",
        order.cost, order.expires_in
    );
    println!(
        "SEND THE VERIFICATION SMS TO: +{}{}",
        order.cc,
        order.phonenumber.expose()
    );

    // Bounded by the provider's own expiry so the harness cannot outlive the order.
    // `SMSPOOL_LIVE_WAIT_SECONDS` shortens the window; it can never extend past expiry.
    let requested = std::env::var("SMSPOOL_LIVE_WAIT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(900);
    let budget = Duration::from_secs(order.expires_in.min(requested));
    let poll = PollOptions::new(Instant::now() + budget, CancellationToken::new())
        .with_intervals(Duration::from_secs(5), Duration::from_secs(15))
        .expect("intervals");

    let outcome = wait_for_sms(&client, &order.order_id, poll).await;

    // Anything other than a genuinely received SMS must attempt a refund. A `message`-only
    // response can be a misclassified still-live order, so printing and returning would abandon
    // both the number and the money.
    let received = match outcome {
        Ok(SmsCheck::Received(received)) => Some(received),
        Ok(other) => {
            println!(
                "TERMINAL WITHOUT SMS: {other:?} terminal={}",
                other.is_terminal()
            );
            None
        }
        Err(error) => {
            println!("NO SMS: {error:?}");
            None
        }
    };

    match received {
        Some(received) => {
            // The point of the capture: which fields the provider actually populates.
            println!(
                "RECEIVED status={:?} sms_present={} full_sms_present={} sms_len={:?} full_sms_len={:?} differ={:?}",
                received.status,
                received.sms.is_some(),
                received.full_sms.is_some(),
                received.sms.as_ref().map(|value| value.expose().len()),
                received.full_sms.as_ref().map(|value| value.expose().len()),
                match (&received.sms, &received.full_sms) {
                    (Some(short), Some(full)) => Some(short.expose() != full.expose()),
                    _ => None,
                }
            );
            if print_code {
                println!(
                    "sms={:?} full_sms={:?}",
                    received.sms.as_ref().map(|value| value.expose()),
                    received.full_sms.as_ref().map(|value| value.expose())
                );
            }
        }
        None => {
            println!("attempting refund");
            let options = CancelOptions::new(
                PollOptions::new(
                    Instant::now() + Duration::from_secs(300),
                    CancellationToken::new(),
                )
                .with_intervals(Duration::from_secs(15), Duration::from_secs(30))
                .expect("intervals"),
            )
            .max_cancel_attempts(10)
            // The live-observed cancellation time lock: HTTP 400 with this exact message and no
            // machine_type, clearing after ~132s. Without this rule the first attempt is rejected
            // and the refund is abandoned. Re-verify for your own account before relying on it.
            .time_lock_rule(
                CancelTimeLockRule::message(
                    StatusCode::BAD_REQUEST,
                    "This phone number cannot be cancelled yet, please try again later!",
                    Duration::from_secs(20),
                )
                .expect("time lock rule"),
            )
            .expected_refund(order.cost);
            match smspool::cancel_with_reconciliation(&client, &order.order_id, options).await {
                Ok(result) => println!("cancel disposition={:?}", result.disposition),
                Err(error) => println!("cancel failed: {error:?}"),
            }
        }
    }
}

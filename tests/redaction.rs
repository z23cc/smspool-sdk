mod support;

use std::time::Duration;

use http::Method;
use serde_json::json;
use smspool::{
    api::{business::BusinessHistoryResponse, sms::CancelAllResponse},
    ActivationToken, AuthMode, BodyMode, Client, Error, EsimCredential, Password, PhoneNumber,
    RedactedValue, RetryPolicy, SafetyClass, SmsText, StatusValue, TransportRequest,
};
use support::{ResponseScript, Script, ScriptedServer};

const API_KEY: &str = "api-key-secret-8df8a";
const PHONE: &str = "phone-secret-15db1";
const SMS_CODE: &str = "sms-code-secret-774b2";
const PASSWORD: &str = "password-secret-1f512";
const VOUCHER: &str = "voucher-secret-a6067";
const ESIM: &str = "esim-secret-98ec3";
const RAW_BODY: &str = "raw-provider-secret-aa8e4";

fn sentinels() -> [&'static str; 7] {
    [API_KEY, PHONE, SMS_CODE, PASSWORD, VOUCHER, ESIM, RAW_BODY]
}

fn assert_redacted(output: &str) {
    for sentinel in sentinels() {
        assert!(
            !output.contains(sentinel),
            "diagnostic output leaked sentinel {sentinel}: {output}"
        );
    }
}

fn client(server: &ScriptedServer, tracing: bool) -> Client {
    Client::builder(API_KEY)
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .retry_policy(
            RetryPolicy::new(1)
                .base_delay(Duration::ZERO)
                .max_delay(Duration::ZERO)
                .jitter_ratio(0.0),
        )
        .tracing(tracing)
        .build()
        .unwrap()
}

fn sensitive_request(safety: SafetyClass) -> TransportRequest {
    TransportRequest::new(
        "sms.purchase",
        Method::POST,
        "/purchase/sms",
        BodyMode::Multipart,
        AuthMode::Bearer,
        safety,
    )
    .body_field("phonenumber", PHONE)
    .body_field("sms", SMS_CODE)
    .body_field("password", PASSWORD)
    .body_field("voucher", VOUCHER)
    .body_field("esim", ESIM)
}

#[test]
fn clients_requests_and_sensitive_wrappers_are_redacted() {
    let builder = Client::builder(API_KEY);
    assert_redacted(&format!("{builder:?}"));
    let client = builder.build().unwrap();
    assert_redacted(&format!("{client:?}"));

    let request = sensitive_request(SafetyClass::PaidMutation).query_field("customer", PHONE);
    assert_redacted(&format!("{request:?}"));

    let values = [
        format!(
            "{:?} {}",
            PhoneNumber::new(PHONE).unwrap(),
            PhoneNumber::new(PHONE).unwrap()
        ),
        format!(
            "{:?} {}",
            SmsText::new(SMS_CODE).unwrap(),
            SmsText::new(SMS_CODE).unwrap()
        ),
        format!(
            "{:?} {}",
            Password::new(PASSWORD).unwrap(),
            Password::new(PASSWORD).unwrap()
        ),
        format!(
            "{:?} {}",
            ActivationToken::new(ESIM).unwrap(),
            ActivationToken::new(ESIM).unwrap()
        ),
        format!(
            "{:?} {}",
            EsimCredential::new(ESIM).unwrap(),
            EsimCredential::new(ESIM).unwrap()
        ),
    ];
    for value in values {
        assert_redacted(&value);
    }
}

#[test]
fn typed_arbitrary_json_fallbacks_are_redacted_by_default() {
    let fallback = RedactedValue::new(json!({"customer_data": RAW_BODY}));
    assert_redacted(&format!("{fallback:?}"));
    assert_eq!(fallback.expose()["customer_data"], RAW_BODY);

    let status: StatusValue =
        serde_json::from_value(json!({"future_status_payload": RAW_BODY})).unwrap();
    assert_redacted(&format!("{status:?}"));

    let history: BusinessHistoryResponse =
        serde_json::from_value(json!({"history": [{"sms": SMS_CODE}]})).unwrap();
    let cancelled: CancelAllResponse = serde_json::from_value(json!({
        "message": "cancelled",
        "refunded_orders": [{"phone": PHONE}]
    }))
    .unwrap();
    assert_redacted(&format!("{history:?} {cancelled:?}"));
}

#[tokio::test]
async fn api_error_debug_display_and_error_chain_omit_provider_customer_data() {
    let server = ScriptedServer::start([Script::Respond(ResponseScript::json(
        400,
        json!({
            "success": 0,
            "type": API_KEY,
            "message": PHONE,
            "errors": [{
                "parameter": SMS_CODE,
                "message": PASSWORD,
                "description": VOUCHER
            }],
            "details": {"activation": ESIM},
            "raw": RAW_BODY
        }),
    ))])
    .await;
    let client = client(&server, false);
    let error = client
        .experimental()
        .raw(sensitive_request(SafetyClass::ReadOnly))
        .await
        .unwrap_err();
    assert!(matches!(&error, Error::Api(_)));

    let mut output = format!("debug={error:?}; display={error}");
    let mut source = std::error::Error::source(&error);
    while let Some(error) = source {
        output.push_str(&format!("; source={error:?}/{error}"));
        source = error.source();
    }
    assert_redacted(&output);
    assert!(output.contains("Api"));
}

#[tokio::test]
async fn outcome_unknown_contains_only_safe_reconciliation_metadata() {
    let server = ScriptedServer::start([Script::Disconnect]).await;
    let client = client(&server, false);
    let error = client
        .experimental()
        .raw(sensitive_request(SafetyClass::PaidMutation))
        .await
        .unwrap_err();
    assert!(matches!(&error, Error::OutcomeUnknown(_)));

    let output = format!("{error:?}\n{error}");
    assert_redacted(&output);
    assert!(output.contains("sms.purchase"));
    assert!(output.contains("OutcomeUnknown"));
}

#[cfg(feature = "tracing")]
mod tracing_redaction {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use super::*;
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CaptureWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            CaptureWriter(self.0.clone())
        }
    }

    #[tokio::test]
    async fn tracing_uses_only_endpoint_attempt_and_delay_metadata() {
        let server = ScriptedServer::start([
            Script::Respond(ResponseScript::json(
                503,
                json!({"success": 0, "raw": RAW_BODY}),
            )),
            Script::Respond(ResponseScript::json(200, json!({"success": 1}))),
        ])
        .await;
        let client = Client::builder(API_KEY)
            .base_url(server.base_url())
            .allow_insecure_http_for_mocking(true)
            .retry_policy(
                RetryPolicy::new(2)
                    .base_delay(Duration::ZERO)
                    .max_delay(Duration::ZERO)
                    .jitter_ratio(0.0),
            )
            .tracing(true)
            .build()
            .unwrap();
        let capture = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .without_time()
            .with_ansi(false)
            .with_writer(capture.clone())
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        let result = client
            .experimental()
            .raw(sensitive_request(SafetyClass::ReadOnly))
            .await;
        drop(guard);
        result.unwrap();

        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
        assert_redacted(&output);
        assert!(output.contains("sms.purchase"));
        assert!(output.contains("attempt"));
        assert!(output.contains("retry"));
    }
}

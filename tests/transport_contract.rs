mod support;

use std::time::{Duration, SystemTime};

use http::Method;
use serde_json::{json, Value};
use smspool::{
    sms::PurchaseSmsRequest, AuthMode, BodyMode, Client, CountryId, Error, OrderId, OutcomeStage,
    RetryPolicy, SafetyClass, ServiceId, TimeoutPhase, TransportErrorKind, TransportRequest,
};
use support::{ResponseScript, Script, ScriptedServer};

fn retry_policy(attempts: usize) -> RetryPolicy {
    RetryPolicy::new(attempts)
        .base_delay(Duration::from_millis(1))
        .max_delay(Duration::from_millis(2))
        .max_retry_after(Duration::from_millis(10))
        .jitter_ratio(0.0)
}

fn client(server: &ScriptedServer) -> Client {
    Client::builder("api-key-wire-sentinel")
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .retry_policy(retry_policy(1))
        .build()
        .unwrap()
}

fn request(
    endpoint: &'static str,
    method: Method,
    path: &'static str,
    body: BodyMode,
    auth: AuthMode,
    safety: SafetyClass,
) -> TransportRequest {
    TransportRequest::new(endpoint, method, path, body, auth, safety)
}

fn ok() -> Script {
    Script::Respond(ResponseScript::json(200, json!({"success": 1})))
}

#[tokio::test]
async fn exact_body_modes_and_authentication_are_preserved() {
    let server = ScriptedServer::start([ok(), ok(), ok(), ok(), ok()]).await;
    let client = client(&server);

    let _: Value = client
        .experimental()
        .raw(
            request(
                "sms.check",
                Method::POST,
                "/sms/check",
                BodyMode::Multipart,
                AuthMode::BearerAndFormKey,
                SafetyClass::ReadOnly,
            )
            .body_field("orderid", "order-42"),
        )
        .await
        .unwrap();
    let _: Value = client
        .experimental()
        .raw(
            request(
                "catalog.countries",
                Method::GET,
                "/country/retrieve_all",
                BodyMode::Multipart,
                AuthMode::Bearer,
                SafetyClass::ReadOnly,
            )
            .body_field("filter", "active"),
        )
        .await
        .unwrap();
    let _: Value = client
        .experimental()
        .raw(
            request(
                "sms.area_codes",
                Method::POST,
                "/request/areacodes",
                BodyMode::FormUrlEncoded,
                AuthMode::FormKey,
                SafetyClass::ReadOnly,
            )
            .body_field("country", "US"),
        )
        .await
        .unwrap();
    let _: Value = client
        .experimental()
        .raw(request(
            "business.users",
            Method::GET,
            "/business/users",
            BodyMode::None,
            AuthMode::Bearer,
            SafetyClass::ReadOnly,
        ))
        .await
        .unwrap();
    let _: Value = client
        .experimental()
        .raw(
            request(
                "raw.public",
                Method::POST,
                "/raw/public",
                BodyMode::RawJson,
                AuthMode::Public,
                SafetyClass::ReadOnly,
            )
            .raw_json(json!({"public": true})),
        )
        .await
        .unwrap();

    let captured = server.requests();
    assert_eq!(captured.len(), 5);

    assert_eq!(captured[0].method, "POST");
    assert_eq!(captured[0].target, "/sms/check");
    assert_eq!(
        captured[0].header("authorization"),
        Some("Bearer api-key-wire-sentinel")
    );
    assert!(captured[0]
        .header("content-type")
        .unwrap()
        .starts_with("multipart/form-data; boundary="));
    let multipart = captured[0].body_text();
    assert!(multipart.contains("name=\"orderid\""));
    assert!(multipart.contains("order-42"));
    assert!(multipart.contains("name=\"key\""));
    assert!(multipart.contains("api-key-wire-sentinel"));

    assert_eq!(captured[1].method, "GET");
    assert_eq!(captured[1].target, "/country/retrieve_all");
    assert!(captured[1]
        .header("content-type")
        .unwrap()
        .starts_with("multipart/form-data; boundary="));

    assert_eq!(captured[2].target, "/request/areacodes");
    assert_eq!(
        captured[2].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(captured[2].header("authorization"), None);
    let form = captured[2].body_text();
    assert!(form.contains("country=US"));
    assert!(form.contains("key=api-key-wire-sentinel"));

    assert_eq!(captured[3].method, "GET");
    assert!(captured[3].body.is_empty());
    assert_eq!(
        captured[3].header("authorization"),
        Some("Bearer api-key-wire-sentinel")
    );

    assert_eq!(captured[4].header("authorization"), None);
    assert_eq!(captured[4].header("content-type"), Some("application/json"));
    assert_eq!(captured[4].body, br#"{"public":true}"#);
    assert!(captured.iter().all(|item| !item.target.contains("api-key")));
}

#[tokio::test]
async fn cross_origin_307_is_not_followed_and_cannot_leak_credentials() {
    let target = ScriptedServer::start([ok()]).await;
    let redirect_url = format!("{}redirected", target.base_url());
    let source = ScriptedServer::start([Script::Respond(
        ResponseScript::json(307, json!({"success": 0, "message": "redirect"}))
            .header("location", redirect_url),
    )])
    .await;
    let client = client(&source);

    let result = client
        .sms()
        .check(&OrderId::new("redirect-order").unwrap())
        .await;
    assert!(matches!(result, Err(Error::Api(_))));
    source.wait_for_requests(1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let source_request = source.requests().pop().unwrap();
    assert_eq!(
        source_request.header("authorization"),
        Some("Bearer api-key-wire-sentinel")
    );
    assert!(source_request.body_text().contains("api-key-wire-sentinel"));
    assert_eq!(
        target.request_count(),
        0,
        "redirect target received credentials"
    );
}

#[tokio::test]
async fn rate_limit_is_classified_from_headers_before_oversized_or_stalled_bodies() {
    let oversized = || {
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0})).declared_content_length(1_024),
        )
    };
    let stalled = || {
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0})).body_delay(Duration::from_millis(300)),
        )
    };
    let server = ScriptedServer::start([oversized(), stalled(), oversized(), stalled()]).await;
    let client = Client::builder("rate-limit-key")
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .max_response_bytes(32)
        .request_timeout(Duration::from_millis(50))
        .retry_policy(retry_policy(1))
        .build()
        .unwrap();

    for (case, safety) in [
        ("read oversized", SafetyClass::ReadOnly),
        ("read stalled", SafetyClass::ReadOnly),
        ("mutation oversized", SafetyClass::Mutation),
        ("mutation stalled", SafetyClass::Mutation),
    ] {
        let result = client
            .experimental()
            .raw(request(
                "test.rate_limit_headers",
                Method::POST,
                "/rate-limited",
                BodyMode::Multipart,
                AuthMode::Bearer,
                safety,
            ))
            .await;
        assert!(
            matches!(result, Err(Error::RateLimited { .. })),
            "{case} should be RateLimited, got {result:?}"
        );
    }
    server.wait_for_requests(4).await;
}

#[tokio::test]
async fn top_level_failure_precedes_model_decode_but_nested_failure_is_ignored() {
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::json(
            200,
            json!({
                "success": 0,
                "type": "invalid_order",
                "message": "not found",
                "errors": [{"parameter": "orderid", "message": "invalid"}],
                "details": {"provider": "value"}
            }),
        )),
        Script::Respond(ResponseScript::json(
            200,
            json!({"success": 1, "nested": {"success": 0}}),
        )),
        Script::Respond(ResponseScript::json(200, json!([{"success": 0}]))),
    ])
    .await;
    let client = client(&server);
    let read = || {
        request(
            "test.read",
            Method::POST,
            "/decode",
            BodyMode::Multipart,
            AuthMode::Bearer,
            SafetyClass::ReadOnly,
        )
    };

    let error = client.experimental().raw(read()).await.unwrap_err();
    match error {
        Error::Api(api) => {
            assert_eq!(api.machine_type(), Some("invalid_order"));
            assert_eq!(api.message(), Some("not found"));
            assert_eq!(api.parameter_errors().len(), 1);
            assert!(api.provider_details().is_some());
            assert_eq!(api.raw().unwrap()["success"], 0);
        }
        other => panic!("unexpected error: {other:?}"),
    }

    let nested = client.experimental().raw(read()).await.unwrap();
    assert_eq!(nested["nested"]["success"], 0);
    let array = client.experimental().raw(read()).await.unwrap();
    assert!(array.is_array());
}

#[tokio::test]
async fn only_read_only_calls_retry_429_and_selected_5xx() {
    let server = ScriptedServer::start([
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0})).header("retry-after", "0"),
        ),
        Script::Respond(ResponseScript::bytes(503, b"malformed-5xx".to_vec())),
        ok(),
        Script::Respond(ResponseScript::json(429, json!({"success": 0}))),
        Script::Respond(ResponseScript::json(503, json!({"success": 0}))),
    ])
    .await;
    let client = Client::builder("retry-key")
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .retry_policy(retry_policy(3))
        .build()
        .unwrap();

    let result: Value = client
        .experimental()
        .raw(request(
            "test.read",
            Method::GET,
            "/retry-read",
            BodyMode::None,
            AuthMode::Bearer,
            SafetyClass::ReadOnly,
        ))
        .await
        .unwrap();
    assert_eq!(result["success"], 1);

    let mutation = client
        .experimental()
        .raw(request(
            "test.mutation",
            Method::POST,
            "/no-retry-429",
            BodyMode::Multipart,
            AuthMode::Bearer,
            SafetyClass::Mutation,
        ))
        .await
        .unwrap_err();
    assert!(matches!(mutation, Error::RateLimited { .. }));

    let paid = client
        .experimental()
        .raw(request(
            "test.paid",
            Method::POST,
            "/no-retry-503",
            BodyMode::Multipart,
            AuthMode::Bearer,
            SafetyClass::PaidMutation,
        ))
        .await
        .unwrap_err();
    assert!(matches!(paid, Error::Api(_)));
    assert_eq!(server.request_count(), 5);
}

#[tokio::test]
async fn retry_after_delta_and_http_date_are_bounded_and_returned() {
    let future = httpdate::fmt_http_date(SystemTime::now() + Duration::from_secs(30));
    let server = ScriptedServer::start([
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0})).header("retry-after", "12"),
        ),
        Script::Respond(
            ResponseScript::json(429, json!({"success": 0})).header("retry-after", future),
        ),
    ])
    .await;
    let client = Client::builder("key")
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .retry_policy(
            RetryPolicy::new(1)
                .max_retry_after(Duration::from_secs(3))
                .jitter_ratio(0.0),
        )
        .build()
        .unwrap();
    let read = || {
        request(
            "test.read",
            Method::GET,
            "/limited",
            BodyMode::None,
            AuthMode::Public,
            SafetyClass::ReadOnly,
        )
    };

    for _ in 0..2 {
        match client.experimental().raw(read()).await.unwrap_err() {
            Error::RateLimited {
                retry_after: Some(delay),
                ..
            } => assert_eq!(delay, Duration::from_secs(3)),
            other => panic!("unexpected error: {other:?}"),
        }
    }
}

#[tokio::test]
async fn connect_before_send_is_ordinary_but_post_send_disconnect_is_unknown() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = listener.local_addr().unwrap();
    drop(listener);
    let connect_client = Client::builder("key")
        .base_url(format!("http://{closed}/"))
        .allow_insecure_http_for_mocking(true)
        .retry_policy(retry_policy(1))
        .build()
        .unwrap();
    let mutation = || {
        request(
            "sms.cancel",
            Method::POST,
            "/sms/cancel",
            BodyMode::Multipart,
            AuthMode::BearerAndFormKey,
            SafetyClass::Mutation,
        )
        .body_field("orderid", "42")
    };

    let error = connect_client
        .experimental()
        .raw(mutation())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::Transport {
            kind: TransportErrorKind::Connect,
            ..
        }
    ));

    let server = ScriptedServer::start([Script::Disconnect]).await;
    let disconnect_client = client(&server);
    let error = disconnect_client
        .experimental()
        .raw(mutation())
        .await
        .unwrap_err();
    match error {
        Error::OutcomeUnknown(unknown) => {
            assert_eq!(unknown.endpoint(), "sms.cancel");
            assert_eq!(unknown.safety(), SafetyClass::Mutation);
            assert_eq!(unknown.stage(), OutcomeStage::Sending);
            assert_eq!(unknown.status(), None);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn decode_failures_are_safe_for_reads_and_unknown_for_mutations() {
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::bytes(200, b"not-json".to_vec())),
        Script::Respond(ResponseScript::bytes(200, b"not-json".to_vec())),
        Script::Respond(ResponseScript::json(200, json!({"other": 1}))),
        Script::Respond(ResponseScript::json(200, json!({"other": 1}))),
    ])
    .await;
    let client = client(&server);
    let make = |safety| {
        request(
            "test.decode",
            Method::POST,
            "/decode",
            BodyMode::Multipart,
            AuthMode::Bearer,
            safety,
        )
    };

    assert!(matches!(
        client.experimental().raw(make(SafetyClass::ReadOnly)).await,
        Err(Error::Decode { path: None, .. })
    ));
    assert!(matches!(
        client.experimental().raw(make(SafetyClass::Mutation)).await,
        Err(Error::OutcomeUnknown(ref value)) if value.stage() == OutcomeStage::JsonDecode
    ));
    match client.catalog().balance().await.unwrap_err() {
        Error::Decode {
            path: Some(path), ..
        } => assert!(!path.is_empty()),
        other => panic!("unexpected error: {other:?}"),
    }
    let purchase = PurchaseSmsRequest::new(
        CountryId::new("US").unwrap(),
        ServiceId::new("service").unwrap(),
    );
    assert!(matches!(
        client.sms().purchase(&purchase).await,
        Err(Error::OutcomeUnknown(ref value)) if value.stage() == OutcomeStage::ModelDecode
    ));
}

#[tokio::test]
async fn declared_and_streamed_bodies_are_bounded_with_mutation_ambiguity() {
    let large = vec![b'x'; 128];
    let server = ScriptedServer::start([
        Script::Respond(ResponseScript::bytes(200, b"{}".to_vec()).declared_content_length(128)),
        Script::Respond(ResponseScript::bytes(200, large.clone()).without_content_length()),
        Script::Respond(ResponseScript::bytes(200, b"{}".to_vec()).declared_content_length(128)),
        Script::Respond(ResponseScript::bytes(200, large).without_content_length()),
    ])
    .await;
    let client = Client::builder("key")
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .max_response_bytes(32)
        .retry_policy(retry_policy(1))
        .build()
        .unwrap();
    let make = |safety| {
        request(
            "test.limit",
            Method::GET,
            "/large",
            BodyMode::None,
            AuthMode::Public,
            safety,
        )
    };

    assert!(matches!(
        client.experimental().raw(make(SafetyClass::ReadOnly)).await,
        Err(Error::ResponseTooLarge { limit: 32, .. })
    ));
    assert!(matches!(
        client.experimental().raw(make(SafetyClass::ReadOnly)).await,
        Err(Error::ResponseTooLarge { limit: 32, .. })
    ));
    assert!(matches!(
        client.experimental().raw(make(SafetyClass::Mutation)).await,
        Err(Error::OutcomeUnknown(ref value)) if value.stage() == OutcomeStage::ResponseHeaders
    ));
    assert!(matches!(
        client.experimental().raw(make(SafetyClass::Mutation)).await,
        Err(Error::OutcomeUnknown(ref value)) if value.stage() == OutcomeStage::ResponseBody
    ));
}

#[tokio::test]
async fn concurrency_wait_request_and_body_timeouts_have_distinct_phases() {
    let server = ScriptedServer::start([
        Script::Respond(
            ResponseScript::json(200, json!({"success": 1}))
                .headers_delay(Duration::from_millis(300)),
        ),
        ok(),
        Script::Hang(Duration::from_millis(300)),
        Script::Respond(
            ResponseScript::json(200, json!({"success": 1})).body_delay(Duration::from_millis(300)),
        ),
    ])
    .await;
    let client = Client::builder("key")
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .max_concurrency(1)
        .concurrency_wait_timeout(Duration::from_millis(20))
        .request_timeout(Duration::from_millis(80))
        .retry_policy(retry_policy(1))
        .build()
        .unwrap();
    let read = || {
        request(
            "test.timeout",
            Method::GET,
            "/timeout",
            BodyMode::None,
            AuthMode::Public,
            SafetyClass::ReadOnly,
        )
    };

    let first_client = client.clone();
    let first = tokio::spawn(async move { first_client.experimental().raw(read()).await });
    server.wait_for_requests(1).await;
    assert!(matches!(
        client.experimental().raw(read()).await,
        Err(Error::Timeout {
            phase: TimeoutPhase::ConcurrencyPermit,
            ..
        })
    ));
    first.abort();
    let _: Value = client.experimental().raw(read()).await.unwrap();

    assert!(matches!(
        client.experimental().raw(read()).await,
        Err(Error::Timeout {
            phase: TimeoutPhase::Request,
            ..
        })
    ));
    assert!(matches!(
        client.experimental().raw(read()).await,
        Err(Error::Timeout {
            phase: TimeoutPhase::ResponseBody,
            ..
        })
    ));
}

#[tokio::test]
async fn concurrency_limit_serializes_in_flight_requests() {
    let delayed = || {
        Script::Respond(
            ResponseScript::json(200, json!({"success": 1}))
                .headers_delay(Duration::from_millis(40)),
        )
    };
    let server = ScriptedServer::start([delayed(), delayed()]).await;
    let client = Client::builder("key")
        .base_url(server.base_url())
        .allow_insecure_http_for_mocking(true)
        .max_concurrency(1)
        .concurrency_wait_timeout(Duration::from_secs(1))
        .retry_policy(retry_policy(1))
        .build()
        .unwrap();
    let make = || {
        request(
            "test.concurrent",
            Method::GET,
            "/concurrent",
            BodyMode::None,
            AuthMode::Public,
            SafetyClass::ReadOnly,
        )
    };

    let first_client = client.clone();
    let first = tokio::spawn(async move { first_client.experimental().raw(make()).await });
    let second_client = client.clone();
    let second = tokio::spawn(async move { second_client.experimental().raw(make()).await });
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(server.max_in_flight(), 1);
}

#[tokio::test]
async fn invalid_modes_and_query_credentials_fail_before_network_io() {
    let server = ScriptedServer::start([]).await;
    let client = client(&server);

    let error = client
        .experimental()
        .raw(
            request(
                "raw.invalid",
                Method::GET,
                "/invalid",
                BodyMode::None,
                AuthMode::Public,
                SafetyClass::ReadOnly,
            )
            .query_field("key", "must-never-enter-url"),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        Error::InvalidRequest { field: "query", .. }
    ));

    let error = client
        .experimental()
        .raw(request(
            "raw.invalid",
            Method::GET,
            "/invalid",
            BodyMode::None,
            AuthMode::FormKey,
            SafetyClass::ReadOnly,
        ))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest { field: "auth", .. }));
    assert_eq!(server.request_count(), 0);
}

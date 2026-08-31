//! Proves that a loopback mock request ignores environment proxy settings.
//!
//! Rust 2024 makes `std::env::set_var` unsafe because mutating the environment races with any
//! concurrent `getenv`. The mutation here is sound only because of an invariant that this file
//! must preserve:
//!
//! 1. **This binary contains exactly one test.** Integration tests run in parallel threads within
//!    a binary, so a second test here would make the mutation racy. Adding one requires moving
//!    the environment manipulation into a child process instead.
//! 2. `#[tokio::test]` defaults to a current-thread runtime, so there are no tokio workers.
//! 3. `support::ScriptedServer` spawns tokio tasks only, never OS threads.
//!
//! Together these mean no other thread can be inside `getenv` while the guards run.

mod support;

use std::{ffi::OsString, time::Duration};

use serde_json::json;
use smspool::{Client, RetryPolicy};
use support::{ResponseScript, Script, ScriptedServer};

struct EnvGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let previous = std::env::var_os(key);
        match value {
            // SAFETY: see the module invariant above. No other thread exists in this process
            // while the environment is mutated.
            Some(value) => unsafe { std::env::set_var(key, value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var(key) },
        }
        Self { key, previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            // SAFETY: as above. Guards are declared before the `Client`, so the client and its
            // connection pool are dropped first and cannot observe the restore.
            Some(value) => unsafe { std::env::set_var(self.key, value) },
            // SAFETY: as above.
            None => unsafe { std::env::remove_var(self.key) },
        }
    }
}

#[tokio::test]
async fn loopback_http_mock_bypasses_environment_proxies() {
    let provider = ScriptedServer::start([Script::Respond(ResponseScript::json(
        200,
        json!({"balance": "1.00"}),
    ))])
    .await;
    let proxy = ScriptedServer::start([Script::Respond(ResponseScript::json(
        200,
        json!({"balance": "999.00"}),
    ))])
    .await;

    let proxy_url = proxy.base_url();
    let _environment = [
        EnvGuard::set("HTTP_PROXY", Some(proxy_url)),
        EnvGuard::set("http_proxy", Some(proxy_url)),
        EnvGuard::set("ALL_PROXY", Some(proxy_url)),
        EnvGuard::set("all_proxy", Some(proxy_url)),
        EnvGuard::set("NO_PROXY", None),
        EnvGuard::set("no_proxy", None),
    ];

    let client = Client::builder("proxy-isolation-key")
        .base_url(provider.base_url())
        .allow_insecure_http_for_mocking(true)
        .retry_policy(RetryPolicy::new(1).jitter_ratio(0.0))
        .build()
        .unwrap();
    let balance = client.catalog().balance().await.unwrap();

    assert_eq!(balance.balance.to_string(), "1.00");
    provider.wait_for_requests(1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(proxy.request_count(), 0, "loopback request traversed proxy");
    let provider_request = provider.requests().pop().unwrap();
    assert_eq!(
        provider_request.header("authorization"),
        Some("Bearer proxy-isolation-key")
    );
}

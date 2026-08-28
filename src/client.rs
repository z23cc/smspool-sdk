use std::{fmt, sync::Arc, time::Duration};

use secrecy::SecretString;
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;
use url::{Host, Url};

use crate::{
    endpoint::{Endpoint, WireRequest},
    error::Error,
    transport::{self, TransportRequest},
};

const DEFAULT_BASE_URL: &str = "https://api.smspool.net/";
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONCURRENCY_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_CONCURRENCY: usize = 32;

fn has_loopback_ip_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        Some(Host::Domain(_)) | None => false,
    }
}

/// Retry configuration for read-only calls. `max_attempts` includes the first request.
#[derive(Clone, Debug)]
pub struct RetryPolicy {
    max_attempts: usize,
    base_delay: Duration,
    max_delay: Duration,
    max_retry_after: Duration,
    jitter_ratio: f64,
}

impl RetryPolicy {
    pub fn new(max_attempts: usize) -> Self {
        Self {
            max_attempts,
            ..Self::default()
        }
    }

    pub fn max_attempts(&self) -> usize {
        self.max_attempts
    }

    pub fn base_delay(mut self, value: Duration) -> Self {
        self.base_delay = value;
        self
    }

    pub fn max_delay(mut self, value: Duration) -> Self {
        self.max_delay = value;
        self
    }

    pub fn max_retry_after(mut self, value: Duration) -> Self {
        self.max_retry_after = value;
        self
    }

    pub fn jitter_ratio(mut self, value: f64) -> Self {
        self.jitter_ratio = value;
        self
    }

    pub fn base_delay_value(&self) -> Duration {
        self.base_delay
    }

    pub fn max_delay_value(&self) -> Duration {
        self.max_delay
    }

    pub fn max_retry_after_value(&self) -> Duration {
        self.max_retry_after
    }

    pub fn jitter_ratio_value(&self) -> f64 {
        self.jitter_ratio
    }

    fn validate(&self) -> Result<(), Error> {
        if self.max_attempts == 0 {
            return Err(Error::InvalidRequest {
                field: "retry_policy.max_attempts",
                reason: "must be greater than zero",
            });
        }
        if !self.jitter_ratio.is_finite() || !(0.0..=1.0).contains(&self.jitter_ratio) {
            return Err(Error::InvalidRequest {
                field: "retry_policy.jitter_ratio",
                reason: "must be finite and between zero and one",
            });
        }
        if self.max_delay < self.base_delay {
            return Err(Error::InvalidRequest {
                field: "retry_policy.max_delay",
                reason: "must be at least the base delay",
            });
        }
        Ok(())
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            max_retry_after: Duration::from_secs(30),
            jitter_ratio: 0.2,
        }
    }
}

/// Cheap-clone, thread-safe SDK client.
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) http: reqwest::Client,
    pub(crate) api_key: SecretString,
    pub(crate) base_url: Url,
    pub(crate) request_timeout: Duration,
    pub(crate) concurrency_wait_timeout: Duration,
    pub(crate) max_response_bytes: usize,
    pub(crate) semaphore: Semaphore,
    pub(crate) max_concurrency: usize,
    pub(crate) retry: RetryPolicy,
    pub(crate) tracing_enabled: bool,
}

impl Client {
    pub fn builder(api_key: impl Into<String>) -> ClientBuilder {
        ClientBuilder::new(api_key)
    }

    /// Execute a low-level request through the same bounded/authenticated pipeline used by APIs.
    ///
    /// This is intentionally descriptor-like rather than a way to send arbitrary absolute URLs.
    pub(crate) async fn execute<T>(&self, request: TransportRequest) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        let (endpoint, wire) = request.into_parts()?;
        self.execute_endpoint(&endpoint, wire).await
    }

    pub(crate) async fn execute_json(
        &self,
        request: TransportRequest,
    ) -> Result<serde_json::Value, Error> {
        self.execute(request).await
    }

    pub(crate) async fn execute_endpoint<T>(
        &self,
        endpoint: &Endpoint,
        wire: WireRequest,
    ) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        transport::execute(self, endpoint, wire).await
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field("request_timeout", &self.inner.request_timeout)
            .field(
                "concurrency_wait_timeout",
                &self.inner.concurrency_wait_timeout,
            )
            .field("max_response_bytes", &self.inner.max_response_bytes)
            .field("max_concurrency", &self.inner.max_concurrency)
            .field("retry", &self.inner.retry)
            .field("tracing_enabled", &self.inner.tracing_enabled)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

pub struct ClientBuilder {
    api_key: String,
    base_url: String,
    allow_insecure_http_for_mocking: bool,
    request_timeout: Duration,
    concurrency_wait_timeout: Duration,
    max_response_bytes: usize,
    max_concurrency: usize,
    retry: RetryPolicy,
    tracing_enabled: bool,
}

impl ClientBuilder {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            allow_insecure_http_for_mocking: false,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            concurrency_wait_timeout: DEFAULT_CONCURRENCY_WAIT_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            retry: RetryPolicy::default(),
            tracing_enabled: false,
        }
    }

    pub fn base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = value.into();
        self
    }

    /// Permit plain HTTP only for explicit local/mock-server use.
    pub fn allow_insecure_http_for_mocking(mut self, value: bool) -> Self {
        self.allow_insecure_http_for_mocking = value;
        self
    }

    pub fn request_timeout(mut self, value: Duration) -> Self {
        self.request_timeout = value;
        self
    }

    pub fn concurrency_wait_timeout(mut self, value: Duration) -> Self {
        self.concurrency_wait_timeout = value;
        self
    }

    pub fn max_response_bytes(mut self, value: usize) -> Self {
        self.max_response_bytes = value;
        self
    }

    pub fn max_concurrency(mut self, value: usize) -> Self {
        self.max_concurrency = value;
        self
    }

    pub fn retry_policy(mut self, value: RetryPolicy) -> Self {
        self.retry = value;
        self
    }

    /// Enable low-cardinality transport tracing when the crate's `tracing` feature is active.
    pub fn tracing(mut self, value: bool) -> Self {
        self.tracing_enabled = value;
        self
    }

    pub fn build(self) -> Result<Client, Error> {
        if self.api_key.trim().is_empty() {
            return Err(Error::InvalidRequest {
                field: "api_key",
                reason: "must not be empty",
            });
        }
        if self.request_timeout.is_zero() {
            return Err(Error::InvalidRequest {
                field: "request_timeout",
                reason: "must be greater than zero",
            });
        }
        if self.concurrency_wait_timeout.is_zero() {
            return Err(Error::InvalidRequest {
                field: "concurrency_wait_timeout",
                reason: "must be greater than zero",
            });
        }
        if self.max_response_bytes == 0 {
            return Err(Error::InvalidRequest {
                field: "max_response_bytes",
                reason: "must be greater than zero",
            });
        }
        if self.max_concurrency == 0 {
            return Err(Error::InvalidRequest {
                field: "max_concurrency",
                reason: "must be greater than zero",
            });
        }
        self.retry.validate()?;

        let mut base_url = Url::parse(&self.base_url).map_err(|_| Error::InvalidRequest {
            field: "base_url",
            reason: "must be an absolute URL",
        })?;
        if base_url.cannot_be_a_base() || base_url.host_str().is_none() {
            return Err(Error::InvalidRequest {
                field: "base_url",
                reason: "must be a hierarchical URL with a host",
            });
        }
        if !base_url.username().is_empty() || base_url.password().is_some() {
            return Err(Error::InvalidRequest {
                field: "base_url",
                reason: "must not contain credentials",
            });
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(Error::InvalidRequest {
                field: "base_url",
                reason: "must not contain a query or fragment",
            });
        }
        match base_url.scheme() {
            "https" => {}
            "http" if self.allow_insecure_http_for_mocking && has_loopback_ip_host(&base_url) => {}
            "http" => {
                return Err(Error::InvalidRequest {
                    field: "base_url",
                    reason: "plain HTTP mocking requires an explicit loopback IP host",
                });
            }
            _ => {
                return Err(Error::InvalidRequest {
                    field: "base_url",
                    reason: "scheme must be HTTPS (or explicit HTTP mocking)",
                });
            }
        }
        let normalized_path = format!("{}/", base_url.path().trim_end_matches('/'));
        base_url.set_path(&normalized_path);

        let http_builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        // Plaintext mock credentials must never traverse an environment/system proxy. Production
        // HTTPS clients retain normal proxy discovery, while loopback-only mocks are direct.
        let http_builder = if base_url.scheme() == "http" {
            http_builder.no_proxy()
        } else {
            http_builder
        };
        let http = http_builder.build().map_err(|_| Error::InvalidRequest {
            field: "http_client",
            reason: "could not build the HTTP client",
        })?;

        Ok(Client {
            inner: Arc::new(Inner {
                http,
                api_key: SecretString::new(self.api_key),
                base_url,
                request_timeout: self.request_timeout,
                concurrency_wait_timeout: self.concurrency_wait_timeout,
                max_response_bytes: self.max_response_bytes,
                semaphore: Semaphore::new(self.max_concurrency),
                max_concurrency: self.max_concurrency,
                retry: self.retry,
                tracing_enabled: self.tracing_enabled,
            }),
        })
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientBuilder")
            .field("api_key", &"[REDACTED]")
            .field("base_url", &self.base_url)
            .field(
                "allow_insecure_http_for_mocking",
                &self.allow_insecure_http_for_mocking,
            )
            .field("request_timeout", &self.request_timeout)
            .field("concurrency_wait_timeout", &self.concurrency_wait_timeout)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("max_concurrency", &self.max_concurrency)
            .field("retry", &self.retry)
            .field("tracing_enabled", &self.tracing_enabled)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_enforces_https_and_normalizes_base_paths() {
        assert!(matches!(
            Client::builder("key")
                .base_url("http://example.com")
                .build(),
            Err(Error::InvalidRequest {
                field: "base_url",
                ..
            })
        ));
        for base in ["http://example.com", "http://192.0.2.1", "http://localhost"] {
            assert!(matches!(
                Client::builder("key")
                    .base_url(base)
                    .allow_insecure_http_for_mocking(true)
                    .build(),
                Err(Error::InvalidRequest {
                    field: "base_url",
                    ..
                })
            ));
        }
        Client::builder("key")
            .base_url("http://[::1]:1234/mock")
            .allow_insecure_http_for_mocking(true)
            .build()
            .unwrap();
        let client = Client::builder("key")
            .base_url("http://127.0.0.1:1234/mock")
            .allow_insecure_http_for_mocking(true)
            .build()
            .unwrap();
        assert_eq!(
            client.inner.base_url.as_str(),
            "http://127.0.0.1:1234/mock/"
        );
    }

    #[test]
    fn builder_rejects_secrets_in_base_url_and_invalid_limits() {
        for base in [
            "https://user:pass@example.com",
            "https://example.com?key=secret",
            "https://example.com/#secret",
        ] {
            assert!(Client::builder("key").base_url(base).build().is_err());
        }
        assert!(Client::builder("key").max_concurrency(0).build().is_err());
        assert!(Client::builder("key")
            .max_response_bytes(0)
            .build()
            .is_err());
        assert!(Client::builder(" ").build().is_err());
    }

    #[test]
    fn client_and_builder_debug_are_secret_safe() {
        let sentinel = "api-key-sentinel-16d9";
        let builder = Client::builder(sentinel);
        assert!(!format!("{builder:?}").contains(sentinel));
        let client = builder.build().unwrap();
        assert!(!format!("{client:?}").contains(sentinel));
    }

    #[test]
    fn client_is_clone_send_sync_static() {
        fn assert_traits<T: Clone + Send + Sync + 'static>() {}
        assert_traits::<Client>();
    }
}

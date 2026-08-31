use std::{
    fmt,
    time::{Duration, SystemTime},
};

use futures_util::StreamExt;
use http::{Method, StatusCode, header::RETRY_AFTER};
use secrecy::ExposeSecret;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    client::Client,
    endpoint::{AuthMode, BodyMode, Endpoint, SafetyClass, WireRequest},
    error::{
        ApiError, Error, OutcomeStage, OutcomeUnknown, ParameterError, RawJson, TimeoutPhase,
        TransportErrorKind,
    },
};

const RECONCILIATION_HINT: &str =
    "query the corresponding status or history endpoint before deciding whether to retry";

/// Low-level request input used by resource modules and contract tests.
///
/// Values are deliberately omitted from `Debug`. Absolute paths and query credentials are
/// rejected before a concurrency permit is acquired.
#[derive(Clone)]
pub struct TransportRequest {
    endpoint: &'static str,
    method: Method,
    path: &'static str,
    body_mode: BodyMode,
    auth: AuthMode,
    safety: SafetyClass,
    body_fields: Vec<(String, String)>,
    query_fields: Vec<(String, String)>,
    raw_json: Option<Value>,
}

impl TransportRequest {
    pub fn new(
        endpoint: &'static str,
        method: Method,
        path: &'static str,
        body_mode: BodyMode,
        auth: AuthMode,
        safety: SafetyClass,
    ) -> Self {
        Self {
            endpoint,
            method,
            path,
            body_mode,
            auth,
            safety,
            body_fields: Vec::new(),
            query_fields: Vec::new(),
            raw_json: None,
        }
    }

    pub fn body_field(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.body_fields.push((name.into(), value.into()));
        self
    }

    pub fn query_field(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_fields.push((name.into(), value.into()));
        self
    }

    pub fn raw_json(mut self, value: Value) -> Self {
        self.raw_json = Some(value);
        self
    }

    pub(crate) fn into_parts(self) -> Result<(Endpoint, WireRequest), Error> {
        let endpoint = Endpoint {
            name: self.endpoint,
            method: self.method,
            path: self.path,
            body_mode: self.body_mode,
            auth: self.auth,
            safety: self.safety,
        };
        let wire = WireRequest {
            body_mode: Some(self.body_mode),
            body_fields: self.body_fields,
            query_fields: self.query_fields,
            raw_json: self.raw_json,
        };
        validate(&endpoint, &wire)?;
        Ok((endpoint, wire))
    }
}

impl fmt::Debug for TransportRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransportRequest")
            .field("endpoint", &self.endpoint)
            .field("method", &self.method)
            .field("path", &self.path)
            .field("body_mode", &self.body_mode)
            .field("auth", &self.auth)
            .field("safety", &self.safety)
            .field("body_field_count", &self.body_fields.len())
            .field("query_field_count", &self.query_fields.len())
            .field("has_raw_json", &self.raw_json.is_some())
            .finish()
    }
}

pub(crate) async fn execute<T>(
    client: &Client,
    endpoint: &Endpoint,
    wire: WireRequest,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    validate(endpoint, &wire)?;
    let url = client
        .inner
        .base_url
        .join(endpoint.path.trim_start_matches('/'))
        .map_err(|_| Error::InvalidRequest {
            field: "path",
            reason: "could not join the relative endpoint path",
        })?;

    let max_attempts = if endpoint.safety == SafetyClass::ReadOnly {
        client.inner.retry.max_attempts()
    } else {
        1
    };

    for attempt in 1..=max_attempts {
        trace_attempt(client, endpoint, attempt);
        match execute_attempt(client, endpoint, &wire, url.clone(), attempt < max_attempts).await {
            Ok(value) => return Ok(value),
            Err(failure) if failure.retryable && attempt < max_attempts => {
                let delay = failure
                    .retry_after
                    .unwrap_or_else(|| backoff_delay(client, attempt));
                trace_retry(client, endpoint, attempt, delay);
                tokio::time::sleep(delay).await;
            }
            Err(failure) => return Err(failure.error),
        }
    }

    unreachable!("validated retry policies always make at least one attempt")
}

struct AttemptFailure {
    error: Error,
    retryable: bool,
    retry_after: Option<Duration>,
}

impl AttemptFailure {
    fn final_error(error: Error) -> Self {
        Self {
            error,
            retryable: false,
            retry_after: None,
        }
    }

    fn retryable(error: Error, retry_after: Option<Duration>) -> Self {
        Self {
            error,
            retryable: true,
            retry_after,
        }
    }
}

async fn acquire_admission(
    client: &Client,
    endpoint: &Endpoint,
) -> Result<tokio::sync::OwnedSemaphorePermit, AttemptFailure> {
    let Some(interval) = client.inner.qps_interval else {
        return tokio::time::timeout(
            client.inner.concurrency_wait_timeout,
            client.inner.semaphore.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
            AttemptFailure::final_error(Error::Timeout {
                endpoint: endpoint.name,
                phase: TimeoutPhase::ConcurrencyPermit,
            })
        })?
        .map_err(|_| {
            AttemptFailure::final_error(Error::Transport {
                endpoint: endpoint.name,
                kind: TransportErrorKind::Other,
            })
        });
    };

    // The QPS deadline is independent from the in-flight concurrency deadline. Do not hold a
    // concurrency permit while waiting for a future start slot; after acquiring a permit, recheck
    // the shared schedule before committing the slot.
    let qps_deadline = tokio::time::Instant::now() + client.inner.qps_wait_timeout;
    loop {
        let next_start = tokio::time::timeout_at(qps_deadline, async {
            client.inner.qps_state.lock().await.next_start
        })
        .await
        .map_err(|_| {
            AttemptFailure::final_error(Error::Timeout {
                endpoint: endpoint.name,
                phase: TimeoutPhase::QpsAdmission,
            })
        })?;
        if let Some(next_start) = next_start {
            if next_start > tokio::time::Instant::now()
                && tokio::time::timeout_at(qps_deadline, tokio::time::sleep_until(next_start))
                    .await
                    .is_err()
            {
                return Err(AttemptFailure::final_error(Error::Timeout {
                    endpoint: endpoint.name,
                    phase: TimeoutPhase::QpsAdmission,
                }));
            }
        }

        let permit = tokio::time::timeout(
            client.inner.concurrency_wait_timeout,
            client.inner.semaphore.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
            AttemptFailure::final_error(Error::Timeout {
                endpoint: endpoint.name,
                phase: TimeoutPhase::ConcurrencyPermit,
            })
        })?
        .map_err(|_| {
            AttemptFailure::final_error(Error::Transport {
                endpoint: endpoint.name,
                kind: TransportErrorKind::Other,
            })
        })?;

        let mut state = tokio::time::timeout_at(qps_deadline, client.inner.qps_state.lock())
            .await
            .map_err(|_| {
                AttemptFailure::final_error(Error::Timeout {
                    endpoint: endpoint.name,
                    phase: TimeoutPhase::QpsAdmission,
                })
            })?;
        let now = tokio::time::Instant::now();
        if state.next_start.is_some_and(|next_start| next_start > now) {
            drop(state);
            drop(permit);
            if now >= qps_deadline {
                return Err(AttemptFailure::final_error(Error::Timeout {
                    endpoint: endpoint.name,
                    phase: TimeoutPhase::QpsAdmission,
                }));
            }
            continue;
        }
        state.next_start = Some(now + interval);
        return Ok(permit);
    }
}

async fn execute_attempt<T>(
    client: &Client,
    endpoint: &Endpoint,
    wire: &WireRequest,
    url: url::Url,
    can_retry: bool,
) -> Result<T, AttemptFailure>
where
    T: DeserializeOwned,
{
    let request =
        build_request(client, endpoint, wire, url).map_err(AttemptFailure::final_error)?;

    let _permit = acquire_admission(client, endpoint).await?;

    let response = match tokio::time::timeout(
        client.inner.request_timeout,
        client.inner.http.execute(request),
    )
    .await
    {
        Err(_) => {
            return Err(send_timeout(endpoint));
        }
        Ok(Err(error)) => {
            return Err(send_error(endpoint, &error));
        }
        Ok(Ok(response)) => response,
    };

    let status = response.status();
    let retry_after = parse_retry_after(response.headers().get(RETRY_AFTER), client);

    // A 429 is fully classified by its status and headers. Never wait for or buffer a provider
    // error body: it may be stalled, unbounded, or contain customer data we do not need.
    if status == StatusCode::TOO_MANY_REQUESTS {
        let error = Error::RateLimited {
            endpoint: endpoint.name,
            retry_after,
        };
        return Err(if endpoint.safety == SafetyClass::ReadOnly {
            AttemptFailure::retryable(error, retry_after)
        } else {
            AttemptFailure::final_error(error)
        });
    }

    if let Some(length) = response.content_length() {
        if length > client.inner.max_response_bytes as u64 {
            return Err(AttemptFailure::final_error(after_delivery_error(
                endpoint,
                OutcomeStage::ResponseHeaders,
                Some(status),
                Error::ResponseTooLarge {
                    endpoint: endpoint.name,
                    limit: client.inner.max_response_bytes,
                },
            )));
        }
    }

    let body = match tokio::time::timeout(
        client.inner.request_timeout,
        read_bounded(response, client.inner.max_response_bytes),
    )
    .await
    {
        Err(_) => {
            return Err(AttemptFailure::final_error(after_delivery_error(
                endpoint,
                OutcomeStage::ResponseBody,
                Some(status),
                Error::Timeout {
                    endpoint: endpoint.name,
                    phase: TimeoutPhase::ResponseBody,
                },
            )));
        }
        Ok(Err(ReadBodyError::TooLarge)) => {
            return Err(AttemptFailure::final_error(after_delivery_error(
                endpoint,
                OutcomeStage::ResponseBody,
                Some(status),
                Error::ResponseTooLarge {
                    endpoint: endpoint.name,
                    limit: client.inner.max_response_bytes,
                },
            )));
        }
        Ok(Err(ReadBodyError::Transport)) => {
            return Err(AttemptFailure::final_error(after_delivery_error(
                endpoint,
                OutcomeStage::ResponseBody,
                Some(status),
                Error::Transport {
                    endpoint: endpoint.name,
                    kind: TransportErrorKind::Body,
                },
            )));
        }
        Ok(Ok(body)) => body,
    };

    let retryable_status = matches!(status.as_u16(), 500 | 502 | 503 | 504)
        && endpoint.safety == SafetyClass::ReadOnly;
    if retryable_status && can_retry {
        return Err(AttemptFailure::retryable(
            Error::Api(ApiError::new(status)),
            None,
        ));
    }

    let value: Value = serde_json::from_slice(&body).map_err(|_| {
        AttemptFailure::final_error(after_delivery_error(
            endpoint,
            OutcomeStage::JsonDecode,
            Some(status),
            Error::Decode {
                endpoint: endpoint.name,
                path: None,
            },
        ))
    })?;

    if !status.is_success() || top_level_failure(&value) {
        return Err(AttemptFailure::final_error(Error::Api(decode_api_error(
            status, value,
        ))));
    }

    serde_path_to_error::deserialize(value).map_err(|error| {
        let path = error.path().to_string();
        AttemptFailure::final_error(after_delivery_error(
            endpoint,
            OutcomeStage::ModelDecode,
            Some(status),
            Error::Decode {
                endpoint: endpoint.name,
                path: (!path.is_empty()).then_some(path),
            },
        ))
    })
}

fn validate(endpoint: &Endpoint, wire: &WireRequest) -> Result<(), Error> {
    if endpoint.name.is_empty() {
        return Err(Error::InvalidRequest {
            field: "endpoint",
            reason: "must not be empty",
        });
    }
    if !endpoint.path.starts_with('/')
        || endpoint.path.starts_with("//")
        || endpoint.path.contains('?')
        || endpoint.path.contains('#')
    {
        return Err(Error::InvalidRequest {
            field: "path",
            reason: "must be a relative path without query or fragment",
        });
    }
    if wire
        .body_mode
        .is_some_and(|mode| mode != endpoint.body_mode)
    {
        return Err(Error::InvalidRequest {
            field: "body_mode",
            reason: "does not match the endpoint descriptor",
        });
    }
    if wire.query_fields.iter().any(|(name, _)| {
        name.eq_ignore_ascii_case("key") || name.eq_ignore_ascii_case("authorization")
    }) {
        return Err(Error::InvalidRequest {
            field: "query",
            reason: "credentials must never be placed in the URL query",
        });
    }
    if wire
        .body_fields
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("key"))
    {
        return Err(Error::InvalidRequest {
            field: "key",
            reason: "the transport injects form authentication",
        });
    }

    match endpoint.body_mode {
        BodyMode::None if !wire.body_fields.is_empty() || wire.raw_json.is_some() => {
            return Err(Error::InvalidRequest {
                field: "body",
                reason: "this endpoint does not accept a body",
            });
        }
        BodyMode::Multipart | BodyMode::FormUrlEncoded if wire.raw_json.is_some() => {
            return Err(Error::InvalidRequest {
                field: "body",
                reason: "raw JSON is incompatible with this body mode",
            });
        }
        BodyMode::RawJson if !wire.body_fields.is_empty() || wire.raw_json.is_none() => {
            return Err(Error::InvalidRequest {
                field: "body",
                reason: "raw JSON mode requires exactly one JSON body",
            });
        }
        _ => {}
    }

    if matches!(
        endpoint.auth,
        AuthMode::FormKey | AuthMode::BearerAndFormKey
    ) && !matches!(
        endpoint.body_mode,
        BodyMode::Multipart | BodyMode::FormUrlEncoded
    ) {
        return Err(Error::InvalidRequest {
            field: "auth",
            reason: "form-key authentication requires a form body mode",
        });
    }
    Ok(())
}

fn build_request(
    client: &Client,
    endpoint: &Endpoint,
    wire: &WireRequest,
    url: url::Url,
) -> Result<reqwest::Request, Error> {
    let mut builder = client.inner.http.request(endpoint.method.clone(), url);
    if !wire.query_fields.is_empty() {
        builder = builder.query(&wire.query_fields);
    }

    if matches!(endpoint.auth, AuthMode::Bearer | AuthMode::BearerAndFormKey) {
        builder = builder.bearer_auth(client.inner.api_key.expose_secret());
    }

    let mut fields = wire.body_fields.clone();
    if matches!(
        endpoint.auth,
        AuthMode::FormKey | AuthMode::BearerAndFormKey
    ) {
        fields.push((
            "key".to_owned(),
            client.inner.api_key.expose_secret().to_owned(),
        ));
    }

    builder = match endpoint.body_mode {
        BodyMode::None => builder,
        BodyMode::Multipart => {
            let form = fields
                .into_iter()
                .fold(reqwest::multipart::Form::new(), |form, (name, value)| {
                    form.text(name, value)
                });
            builder.multipart(form)
        }
        BodyMode::FormUrlEncoded => builder.form(&fields),
        BodyMode::RawJson => {
            let encoded = serde_json::to_vec(wire.raw_json.as_ref().expect("validated raw JSON"))
                .map_err(|_| Error::InvalidRequest {
                field: "body",
                reason: "could not encode JSON",
            })?;
            builder
                .header(http::header::CONTENT_TYPE, "application/json")
                .body(encoded)
        }
    };

    builder.build().map_err(|_| Error::InvalidRequest {
        field: "request",
        reason: "could not construct the HTTP request",
    })
}

enum ReadBodyError {
    TooLarge,
    Transport,
}

async fn read_bounded(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, ReadBodyError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ReadBodyError::Transport)?;
        let new_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(ReadBodyError::TooLarge)?;
        if new_len > limit {
            return Err(ReadBodyError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn send_timeout(endpoint: &Endpoint) -> AttemptFailure {
    if endpoint.safety == SafetyClass::ReadOnly {
        AttemptFailure::retryable(
            Error::Timeout {
                endpoint: endpoint.name,
                phase: TimeoutPhase::Request,
            },
            None,
        )
    } else {
        AttemptFailure::final_error(unknown(endpoint, OutcomeStage::Sending, None))
    }
}

fn send_error(endpoint: &Endpoint, error: &reqwest::Error) -> AttemptFailure {
    if error.is_connect() {
        return if endpoint.safety == SafetyClass::ReadOnly {
            AttemptFailure::retryable(
                Error::Transport {
                    endpoint: endpoint.name,
                    kind: TransportErrorKind::Connect,
                },
                None,
            )
        } else {
            AttemptFailure::final_error(Error::Transport {
                endpoint: endpoint.name,
                kind: TransportErrorKind::Connect,
            })
        };
    }

    if endpoint.safety != SafetyClass::ReadOnly {
        return AttemptFailure::final_error(unknown(endpoint, OutcomeStage::Sending, None));
    }

    let error = if error.is_timeout() {
        Error::Timeout {
            endpoint: endpoint.name,
            phase: TimeoutPhase::Request,
        }
    } else {
        Error::Transport {
            endpoint: endpoint.name,
            kind: if error.is_request() {
                TransportErrorKind::Request
            } else if error.is_body() {
                TransportErrorKind::Body
            } else {
                TransportErrorKind::Connection
            },
        }
    };
    AttemptFailure::retryable(error, None)
}

fn after_delivery_error(
    endpoint: &Endpoint,
    stage: OutcomeStage,
    status: Option<StatusCode>,
    read_only_error: Error,
) -> Error {
    if endpoint.safety == SafetyClass::ReadOnly {
        read_only_error
    } else {
        unknown(endpoint, stage, status)
    }
}

fn unknown(endpoint: &Endpoint, stage: OutcomeStage, status: Option<StatusCode>) -> Error {
    Error::OutcomeUnknown(OutcomeUnknown::new(
        endpoint.name,
        endpoint.safety,
        stage,
        status,
        RECONCILIATION_HINT,
    ))
}

fn top_level_failure(value: &Value) -> bool {
    let Some(success) = value.as_object().and_then(|object| object.get("success")) else {
        return false;
    };
    matches!(success, Value::Bool(false))
        || success.as_i64() == Some(0)
        || success.as_str() == Some("0")
}

fn decode_api_error(status: StatusCode, value: Value) -> ApiError {
    let mut error = ApiError::new(status);
    if let Some(object) = value.as_object() {
        if let Some(machine_type) = ["type", "error_type", "code"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(Value::as_str))
        {
            error = error.with_machine_type(machine_type);
        }
        if let Some(message) = ["message", "error", "description"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(Value::as_str))
        {
            error = error.with_message(message);
        }
        if let Some(errors) = object.get("errors").and_then(Value::as_array) {
            let parameter_errors = errors
                .iter()
                .filter_map(Value::as_object)
                .map(|item| {
                    ParameterError::new(
                        item.get("parameter")
                            .or_else(|| item.get("param"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        item.get("message")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        item.get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    )
                })
                .collect();
            error = error.with_parameter_errors(parameter_errors);
        }
        if let Some(details) = object.get("details").or_else(|| object.get("data")) {
            error = error.with_provider_details(details.clone());
        }
    }
    error.with_raw(RawJson::new(value))
}

fn parse_retry_after(value: Option<&http::HeaderValue>, client: &Client) -> Option<Duration> {
    let text = value?.to_str().ok()?.trim();
    let parsed = if let Ok(seconds) = text.parse::<u64>() {
        Duration::from_secs(seconds)
    } else {
        let deadline = httpdate::parse_http_date(text).ok()?;
        deadline
            .duration_since(SystemTime::now())
            .unwrap_or_default()
    };
    Some(parsed.min(client.inner.retry.max_retry_after_value()))
}

fn backoff_delay(client: &Client, attempt: usize) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let multiplier = 2_u32.saturating_pow(exponent.min(31));
    let base = client
        .inner
        .retry
        .base_delay_value()
        .saturating_mul(multiplier)
        .min(client.inner.retry.max_delay_value());
    let jitter = client.inner.retry.jitter_ratio_value();
    if jitter == 0.0 || base.is_zero() {
        return base;
    }
    let factor = (1.0 - jitter) + rand::random::<f64>() * (2.0 * jitter);
    base.mul_f64(factor)
}

fn trace_attempt(client: &Client, endpoint: &Endpoint, attempt: usize) {
    #[cfg(feature = "tracing")]
    if client.inner.tracing_enabled {
        tracing::debug!(endpoint = endpoint.name, attempt, "SMSPool request attempt");
    }
    #[cfg(not(feature = "tracing"))]
    let _ = (client, endpoint, attempt);
}

fn trace_retry(client: &Client, endpoint: &Endpoint, attempt: usize, delay: Duration) {
    #[cfg(feature = "tracing")]
    if client.inner.tracing_enabled {
        tracing::debug!(
            endpoint = endpoint.name,
            attempt,
            delay_ms = delay.as_millis(),
            "SMSPool read-only retry"
        );
    }
    #[cfg(not(feature = "tracing"))]
    let _ = (client, endpoint, attempt, delay);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_top_level_failure_is_classified() {
        assert!(top_level_failure(&serde_json::json!({"success": 0})));
        assert!(top_level_failure(&serde_json::json!({"success": false})));
        assert!(top_level_failure(&serde_json::json!({"success": "0"})));
        assert!(!top_level_failure(
            &serde_json::json!({"success": 1, "nested": {"success": 0}})
        ));
        assert!(!top_level_failure(&serde_json::json!([{"success": 0}])));
    }

    #[test]
    fn transport_request_debug_omits_all_values() {
        let sentinel = "customer-secret-93f1";
        let request = TransportRequest::new(
            "test.read",
            Method::POST,
            "/test",
            BodyMode::RawJson,
            AuthMode::Public,
            SafetyClass::ReadOnly,
        )
        .query_field("lookup", sentinel)
        .raw_json(serde_json::json!({"phone": sentinel}));
        assert!(!format!("{request:?}").contains(sentinel));
    }
}

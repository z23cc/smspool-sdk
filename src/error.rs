use std::{fmt, time::Duration};

use http::StatusCode;
use serde_json::Value;

use crate::endpoint::SafetyClass;

/// Top-level SDK error. Default formatting is deliberately data-free.
#[non_exhaustive]
pub enum Error {
    Api(ApiError),
    RateLimited {
        endpoint: &'static str,
        retry_after: Option<Duration>,
    },
    Transport {
        endpoint: &'static str,
        kind: TransportErrorKind,
    },
    Timeout {
        endpoint: &'static str,
        phase: TimeoutPhase,
    },
    ResponseTooLarge {
        endpoint: &'static str,
        limit: usize,
    },
    Decode {
        endpoint: &'static str,
        path: Option<String>,
    },
    InvalidRequest {
        field: &'static str,
        reason: &'static str,
    },
    UnsupportedOperation {
        endpoint: &'static str,
        reason: UnsupportedReason,
    },
    OutcomeUnknown(OutcomeUnknown),
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(error) => formatter.debug_tuple("Api").field(error).finish(),
            Self::RateLimited {
                endpoint,
                retry_after,
            } => formatter
                .debug_struct("RateLimited")
                .field("endpoint", endpoint)
                .field("retry_after", retry_after)
                .finish(),
            Self::Transport { endpoint, kind } => formatter
                .debug_struct("Transport")
                .field("endpoint", endpoint)
                .field("kind", kind)
                .finish(),
            Self::Timeout { endpoint, phase } => formatter
                .debug_struct("Timeout")
                .field("endpoint", endpoint)
                .field("phase", phase)
                .finish(),
            Self::ResponseTooLarge { endpoint, limit } => formatter
                .debug_struct("ResponseTooLarge")
                .field("endpoint", endpoint)
                .field("limit", limit)
                .finish(),
            Self::Decode { endpoint, path } => formatter
                .debug_struct("Decode")
                .field("endpoint", endpoint)
                .field("path", path)
                .finish(),
            Self::InvalidRequest { field, reason } => formatter
                .debug_struct("InvalidRequest")
                .field("field", field)
                .field("reason", reason)
                .finish(),
            Self::UnsupportedOperation { endpoint, reason } => formatter
                .debug_struct("UnsupportedOperation")
                .field("endpoint", endpoint)
                .field("reason", reason)
                .finish(),
            Self::OutcomeUnknown(error) => formatter
                .debug_tuple("OutcomeUnknown")
                .field(error)
                .finish(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api(error) => error.fmt(formatter),
            Self::RateLimited { endpoint, .. } => {
                write!(formatter, "rate limited by endpoint {endpoint}")
            }
            Self::Transport { endpoint, kind } => {
                write!(formatter, "transport error for endpoint {endpoint}: {kind}")
            }
            Self::Timeout { endpoint, phase } => {
                write!(formatter, "timeout during {phase} for endpoint {endpoint}")
            }
            Self::ResponseTooLarge { endpoint, limit } => write!(
                formatter,
                "response from endpoint {endpoint} exceeded the {limit}-byte limit"
            ),
            Self::Decode { endpoint, path } => match path {
                Some(path) => write!(formatter, "could not decode endpoint {endpoint} at {path}"),
                None => write!(formatter, "could not decode endpoint {endpoint}"),
            },
            Self::InvalidRequest { field, reason } => {
                write!(formatter, "invalid request field {field}: {reason}")
            }
            Self::UnsupportedOperation { endpoint, reason } => {
                write!(formatter, "endpoint {endpoint} is unsupported: {reason}")
            }
            Self::OutcomeUnknown(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnsupportedReason {
    ResponseNotSuitableForBufferedSdk,
}

impl fmt::Display for UnsupportedReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResponseNotSuitableForBufferedSdk => {
                formatter.write_str("the live response is not suitable for bounded buffering")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TransportErrorKind {
    Connect,
    Connection,
    Request,
    Body,
    Other,
}

impl fmt::Display for TransportErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Connect => "connect",
            Self::Connection => "connection",
            Self::Request => "request",
            Self::Body => "body",
            Self::Other => "other",
        };
        formatter.write_str(label)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TimeoutPhase {
    ConcurrencyPermit,
    QpsAdmission,
    Request,
    ResponseBody,
}

impl fmt::Display for TimeoutPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::ConcurrencyPermit => "concurrency permit wait",
            Self::QpsAdmission => "QPS admission",
            Self::Request => "request",
            Self::ResponseBody => "response body",
        };
        formatter.write_str(label)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OutcomeStage {
    Sending,
    ResponseHeaders,
    ResponseBody,
    JsonDecode,
    ModelDecode,
}

/// A non-idempotent operation may have reached the provider, so callers must reconcile it.
#[derive(Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct OutcomeUnknown {
    endpoint: &'static str,
    safety: SafetyClass,
    stage: OutcomeStage,
    status: Option<StatusCode>,
    reconciliation_hint: &'static str,
}

impl OutcomeUnknown {
    #[allow(dead_code)] // Constructed by the transport phase after a possibly delivered mutation.
    pub(crate) fn new(
        endpoint: &'static str,
        safety: SafetyClass,
        stage: OutcomeStage,
        status: Option<StatusCode>,
        reconciliation_hint: &'static str,
    ) -> Self {
        debug_assert!(safety != SafetyClass::ReadOnly);
        Self {
            endpoint,
            safety,
            stage,
            status,
            reconciliation_hint,
        }
    }

    pub fn endpoint(&self) -> &'static str {
        self.endpoint
    }

    pub fn safety(&self) -> SafetyClass {
        self.safety
    }

    pub fn stage(&self) -> OutcomeStage {
        self.stage
    }

    pub fn status(&self) -> Option<StatusCode> {
        self.status
    }

    pub fn reconciliation_hint(&self) -> &'static str {
        self.reconciliation_hint
    }
}

impl fmt::Debug for OutcomeUnknown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutcomeUnknown")
            .field("endpoint", &self.endpoint)
            .field("safety", &self.safety)
            .field("stage", &self.stage)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for OutcomeUnknown {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "outcome of {} is unknown after {:?}; reconciliation is required",
            self.endpoint, self.stage
        )
    }
}

/// Provider error response, retained for explicit inspection but redacted by default.
#[derive(Clone)]
#[non_exhaustive]
pub struct ApiError {
    inner: Box<ApiErrorInner>,
}

#[derive(Clone)]
struct ApiErrorInner {
    status: StatusCode,
    machine_type: Option<String>,
    message: Option<String>,
    parameter_errors: Vec<ParameterError>,
    provider_details: Option<Value>,
    raw: Option<RawJson>,
}

impl ApiError {
    pub fn new(status: StatusCode) -> Self {
        Self {
            inner: Box::new(ApiErrorInner {
                status,
                machine_type: None,
                message: None,
                parameter_errors: Vec::new(),
                provider_details: None,
                raw: None,
            }),
        }
    }

    pub fn status(&self) -> StatusCode {
        self.inner.status
    }

    pub fn machine_type(&self) -> Option<&str> {
        self.inner.machine_type.as_deref()
    }

    pub fn message(&self) -> Option<&str> {
        self.inner.message.as_deref()
    }

    pub fn parameter_errors(&self) -> &[ParameterError] {
        &self.inner.parameter_errors
    }

    pub fn provider_details(&self) -> Option<&Value> {
        self.inner.provider_details.as_ref()
    }

    pub fn raw(&self) -> Option<&Value> {
        self.inner.raw.as_ref().map(RawJson::expose)
    }

    pub fn with_machine_type(mut self, machine_type: impl Into<String>) -> Self {
        self.inner.machine_type = Some(machine_type.into());
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.inner.message = Some(message.into());
        self
    }

    pub fn with_parameter_errors(mut self, errors: Vec<ParameterError>) -> Self {
        self.inner.parameter_errors = errors;
        self
    }

    pub fn with_provider_details(mut self, details: Value) -> Self {
        self.inner.provider_details = Some(details);
        self
    }

    pub fn with_raw(mut self, raw: RawJson) -> Self {
        self.inner.raw = Some(raw);
        self
    }
}

impl fmt::Debug for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiError")
            .field("status", &self.inner.status)
            .field("has_machine_type", &self.inner.machine_type.is_some())
            .field("has_message", &self.inner.message.is_some())
            .field("parameter_error_count", &self.inner.parameter_errors.len())
            .field(
                "has_provider_details",
                &self.inner.provider_details.is_some(),
            )
            .field("has_raw", &self.inner.raw.is_some())
            .finish()
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "SMSPool API error with HTTP status {}",
            self.inner.status
        )
    }
}

#[derive(Clone)]
#[non_exhaustive]
pub struct ParameterError {
    parameter: Option<String>,
    message: Option<String>,
    description: Option<String>,
}

impl ParameterError {
    pub fn new(
        parameter: Option<String>,
        message: Option<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            parameter,
            message,
            description,
        }
    }

    pub fn parameter(&self) -> Option<&str> {
        self.parameter.as_deref()
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl fmt::Debug for ParameterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParameterError")
            .field("has_parameter", &self.parameter.is_some())
            .field("has_message", &self.message.is_some())
            .field("has_description", &self.description.is_some())
            .finish()
    }
}

/// Bounded raw provider JSON. Callers must opt in through [`RawJson::expose`].
#[derive(Clone)]
pub struct RawJson(Value);

impl RawJson {
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &Value {
        &self.0
    }
}

impl fmt::Debug for RawJson {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RawJson([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_default_formatting_is_redacted() {
        let sentinel = "secret-provider-payload-7d93";
        let error = ApiError::new(StatusCode::BAD_REQUEST)
            .with_machine_type(sentinel)
            .with_message(sentinel)
            .with_parameter_errors(vec![ParameterError::new(
                Some("phone".into()),
                Some(sentinel.into()),
                Some(sentinel.into()),
            )])
            .with_provider_details(serde_json::json!({"detail": sentinel}))
            .with_raw(RawJson::new(serde_json::json!({"sms": sentinel})));

        assert!(!format!("{error:?}").contains(sentinel));
        assert!(!error.to_string().contains(sentinel));
        assert_eq!(error.machine_type(), Some(sentinel));
        assert_eq!(error.message(), Some(sentinel));
        assert_eq!(
            error
                .raw()
                .and_then(|value| value.get("sms"))
                .and_then(Value::as_str),
            Some(sentinel)
        );
    }

    #[test]
    fn outcome_unknown_omits_the_reconciliation_hint_from_debug() {
        let sentinel = "reconcile-with-sensitive-order-data";
        let error = OutcomeUnknown::new(
            "sms.purchase",
            SafetyClass::PaidMutation,
            OutcomeStage::ResponseBody,
            Some(StatusCode::OK),
            sentinel,
        );
        assert!(!format!("{error:?}").contains(sentinel));
        assert!(!error.to_string().contains(sentinel));
        assert_eq!(error.reconciliation_hint(), sentinel);
    }
}

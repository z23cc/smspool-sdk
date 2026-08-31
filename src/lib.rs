//! Asynchronous Rust SDK for the SMSPool API.
//!
//! Stable Catalog, Pricing, and core SMS operations share one bounded transport. Less certain
//! collection-derived operations are isolated under [`Client::experimental`].

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

/// Compiles the README's Rust examples as doctests so they cannot silently rot.
///
/// Doc-only; it is not part of the public API surface at runtime.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeDoctests;

pub mod api;
pub mod client;
#[allow(dead_code)]
pub(crate) mod de;
#[allow(dead_code)]
pub(crate) mod endpoint;
pub mod error;
pub mod poll;
pub mod transport;
pub mod types;

pub use api::{catalog, pricing, sms, ExperimentalApi};
pub use client::{Client, ClientBuilder, RetryPolicy};
pub use endpoint::{AuthMode, BodyMode, SafetyClass};
pub use error::{
    ApiError, Error, OutcomeStage, OutcomeUnknown, ParameterError, RawJson, TimeoutPhase,
    TransportErrorKind, UnsupportedReason,
};
pub use poll::{
    cancel_with_reconciliation, wait_for_code_with, wait_for_sms, ActiveObservation,
    ActiveOrdersWatcher, BalanceObservation, CancelOptions, CancelTimeLockRule,
    CancelWorkflowError, CancellationDisposition, CancellationResult, CheckObservation,
    CodePollResult, ExpectedRefundMatch, PollError, PollOptions, PollOptionsError, WatchError,
};
pub use transport::TransportRequest;
pub use types::{
    ActivationToken, BusinessUserId, Cents, CountryId, Days, DecimalValue, DecodedJson,
    EsimCredential, Hours, InvalidValue, Money, OrderId, Password, PhoneNumber, PlanId, PoolId,
    PreorderId, RawFormValue, RedactedValue, RentalCode, RentalId, Seconds, ServiceId,
    SignedMoneyDelta, SmsText, StatusValue, TransactionId, UnixTimestamp, VendorDateTime,
};

//! Asynchronous Rust SDK for the SMSPool API.
//!
//! Stable Catalog, Pricing, and core SMS operations share one bounded transport. Less certain
//! collection-derived operations are isolated under [`Client::experimental`].

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

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
    TransportErrorKind,
};
pub use poll::{
    wait_for_code_with, wait_for_sms, ActiveOrdersWatcher, CodePollResult, PollError, PollOptions,
    PollOptionsError, WatchError,
};
pub use transport::TransportRequest;
pub use types::{
    ActivationToken, BusinessUserId, Cents, CountryId, Days, DecimalValue, DecodedJson,
    EsimCredential, Hours, InvalidValue, Money, OrderId, Password, PhoneNumber, PlanId, PoolId,
    PreorderId, RawFormValue, RedactedValue, RentalCode, RentalId, Seconds, ServiceId, SmsText,
    StatusValue, TransactionId, UnixTimestamp, VendorDateTime,
};

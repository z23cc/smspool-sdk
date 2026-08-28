//! In-memory polling workflows built on the stable SMS API.
//!
//! These helpers deliberately own no durable state. Applications must persist order state and
//! reconcile restarts themselves.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    time::Duration,
};

use rand::Rng as _;
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

use crate::{
    api::sms::{ActiveOrder, SmsCheck},
    Client, Error, OrderId,
};

const DEFAULT_BASE_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_MAX_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_JITTER_RATIO: f64 = 0.2;

/// Deadline, cancellation, and delay policy shared by polling workflows.
#[derive(Clone, Debug)]
pub struct PollOptions {
    deadline: Instant,
    base_interval: Duration,
    max_interval: Duration,
    jitter_ratio: f64,
    cancellation: CancellationToken,
}

impl PollOptions {
    /// Creates polling options with a required absolute Tokio deadline.
    pub fn new(deadline: Instant, cancellation: CancellationToken) -> Self {
        Self {
            deadline,
            base_interval: DEFAULT_BASE_INTERVAL,
            max_interval: DEFAULT_MAX_INTERVAL,
            jitter_ratio: DEFAULT_JITTER_RATIO,
            cancellation,
        }
    }

    /// Sets the initial and maximum polling intervals.
    pub fn with_intervals(
        mut self,
        base_interval: Duration,
        max_interval: Duration,
    ) -> Result<Self, PollOptionsError> {
        if base_interval.is_zero() {
            return Err(PollOptionsError::new(
                "base_interval",
                "must be greater than zero",
            ));
        }
        if max_interval < base_interval {
            return Err(PollOptionsError::new(
                "max_interval",
                "must be at least the base interval",
            ));
        }
        self.base_interval = base_interval;
        self.max_interval = max_interval;
        Ok(self)
    }

    /// Sets bounded proportional jitter. Zero is useful for deterministic tests.
    pub fn with_jitter_ratio(mut self, jitter_ratio: f64) -> Result<Self, PollOptionsError> {
        if !jitter_ratio.is_finite() || !(0.0..=1.0).contains(&jitter_ratio) {
            return Err(PollOptionsError::new(
                "jitter_ratio",
                "must be finite and between zero and one",
            ));
        }
        self.jitter_ratio = jitter_ratio;
        Ok(self)
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn base_interval(&self) -> Duration {
        self.base_interval
    }

    pub fn max_interval(&self) -> Duration {
        self.max_interval
    }

    pub fn jitter_ratio(&self) -> f64 {
        self.jitter_ratio
    }

    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancellation
    }
}

/// Local configuration error detected before a workflow is created.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PollOptionsError {
    field: &'static str,
    reason: &'static str,
}

impl PollOptionsError {
    const fn new(field: &'static str, reason: &'static str) -> Self {
        Self { field, reason }
    }

    pub const fn field(&self) -> &'static str {
        self.field
    }

    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for PollOptionsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid polling option {}: {}",
            self.field, self.reason
        )
    }
}

impl std::error::Error for PollOptionsError {}

/// Failure from a single-order polling workflow.
#[non_exhaustive]
pub enum PollError {
    Client(Error),
    Deadline { last_observed: Option<SmsCheck> },
    Cancelled { last_observed: Option<SmsCheck> },
}

impl PollError {
    pub fn last_observed(&self) -> Option<&SmsCheck> {
        match self {
            Self::Client(_) => None,
            Self::Deadline { last_observed } | Self::Cancelled { last_observed } => {
                last_observed.as_ref()
            }
        }
    }
}

impl fmt::Debug for PollError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => formatter.debug_tuple("Client").field(error).finish(),
            Self::Deadline { last_observed } => formatter
                .debug_struct("Deadline")
                .field("has_last_observed", &last_observed.is_some())
                .finish(),
            Self::Cancelled { last_observed } => formatter
                .debug_struct("Cancelled")
                .field("has_last_observed", &last_observed.is_some())
                .finish(),
        }
    }
}

impl fmt::Display for PollError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => write!(formatter, "SMS polling failed: {error}"),
            Self::Deadline { .. } => formatter.write_str("SMS polling deadline elapsed"),
            Self::Cancelled { .. } => formatter.write_str("SMS polling was cancelled"),
        }
    }
}

impl std::error::Error for PollError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Deadline { .. } | Self::Cancelled { .. } => None,
        }
    }
}

/// Terminal SMS snapshot plus the caller-defined extraction result.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CodePollResult<T> {
    pub sms: SmsCheck,
    pub code: Option<T>,
}

/// Polls until the provider returns a received or terminated SMS snapshot.
///
/// Cancellation while an in-flight read is safe: `sms.check` is a read-only endpoint. The helper
/// retains only the latest snapshot and performs no persistence.
pub async fn wait_for_sms(
    client: &Client,
    order_id: &OrderId,
    options: PollOptions,
) -> Result<SmsCheck, PollError> {
    let mut last_observed = None;
    let mut interval = options.base_interval;

    loop {
        preflight(&options, &last_observed)?;

        let sms_api = client.sms();
        let checked = tokio::select! {
            biased;
            _ = options.cancellation.cancelled() => {
                return Err(PollError::Cancelled { last_observed });
            }
            _ = sleep_until(options.deadline) => {
                return Err(PollError::Deadline { last_observed });
            }
            result = sms_api.check(order_id) => result,
        };

        let retry_after = match checked {
            Ok(snapshot @ (SmsCheck::Received(_) | SmsCheck::Terminated(_))) => {
                return Ok(snapshot);
            }
            Ok(snapshot @ SmsCheck::Pending(_)) => {
                last_observed = Some(snapshot);
                None
            }
            Err(Error::RateLimited { retry_after, .. }) => retry_after,
            Err(error) => return Err(PollError::Client(error)),
        };

        let normal_delay = interval;
        interval = advance_interval(interval, options.max_interval);
        let delay = polling_delay(&options, normal_delay, retry_after);
        sleep_poll_delay(&options, delay, last_observed.clone()).await?;
    }
}

/// Polls for SMS and applies only the caller's extraction policy.
///
/// The extractor receives explicitly exposed SMS text. `full_sms` is preferred when present;
/// otherwise the shorter `sms` field is used. A terminated response, or a received message for
/// which the extractor returns `None`, produces `code: None` rather than guessing a code.
pub async fn wait_for_code_with<T, F>(
    client: &Client,
    order_id: &OrderId,
    options: PollOptions,
    extractor: F,
) -> Result<CodePollResult<T>, PollError>
where
    F: FnOnce(&str) -> Option<T>,
{
    let sms = wait_for_sms(client, order_id, options).await?;
    let code = match &sms {
        SmsCheck::Received(received) => received
            .full_sms
            .as_ref()
            .or(received.sms.as_ref())
            .and_then(|text| extractor(text.expose())),
        SmsCheck::Pending(_) | SmsCheck::Terminated(_) => None,
    };
    Ok(CodePollResult { sms, code })
}

fn preflight(options: &PollOptions, last_observed: &Option<SmsCheck>) -> Result<(), PollError> {
    if options.cancellation.is_cancelled() {
        return Err(PollError::Cancelled {
            last_observed: last_observed.clone(),
        });
    }
    if Instant::now() >= options.deadline {
        return Err(PollError::Deadline {
            last_observed: last_observed.clone(),
        });
    }
    Ok(())
}

async fn sleep_poll_delay(
    options: &PollOptions,
    delay: Duration,
    last_observed: Option<SmsCheck>,
) -> Result<(), PollError> {
    let wake_at = capped_wake_at(delay, options.deadline);
    tokio::select! {
        biased;
        _ = options.cancellation.cancelled() => Err(PollError::Cancelled { last_observed }),
        _ = sleep_until(options.deadline) => Err(PollError::Deadline { last_observed }),
        _ = sleep_until(wake_at) => Ok(()),
    }
}

/// Failure from the bounded active-order watcher.
#[non_exhaustive]
pub enum WatchError {
    Client(Error),
    Deadline {
        last_observed: Option<Box<ActiveOrder>>,
    },
    Cancelled {
        last_observed: Option<Box<ActiveOrder>>,
    },
    TrackingLimitExceeded {
        limit: usize,
        observed: usize,
    },
}

impl WatchError {
    pub fn last_observed(&self) -> Option<&ActiveOrder> {
        match self {
            Self::Deadline { last_observed } | Self::Cancelled { last_observed } => {
                last_observed.as_deref()
            }
            Self::Client(_) | Self::TrackingLimitExceeded { .. } => None,
        }
    }
}

impl fmt::Debug for WatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => formatter.debug_tuple("Client").field(error).finish(),
            Self::Deadline { last_observed } => formatter
                .debug_struct("Deadline")
                .field("has_last_observed", &last_observed.is_some())
                .finish(),
            Self::Cancelled { last_observed } => formatter
                .debug_struct("Cancelled")
                .field("has_last_observed", &last_observed.is_some())
                .finish(),
            Self::TrackingLimitExceeded { limit, observed } => formatter
                .debug_struct("TrackingLimitExceeded")
                .field("limit", limit)
                .field("observed", observed)
                .finish(),
        }
    }
}

impl fmt::Display for WatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Client(error) => write!(formatter, "active-order polling failed: {error}"),
            Self::Deadline { .. } => formatter.write_str("active-order watcher deadline elapsed"),
            Self::Cancelled { .. } => formatter.write_str("active-order watcher was cancelled"),
            Self::TrackingLimitExceeded { limit, observed } => write!(
                formatter,
                "active-order watcher observed {observed} orders, exceeding its {limit}-order limit"
            ),
        }
    }
}

impl std::error::Error for WatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Client(error) => Some(error),
            Self::Deadline { .. } | Self::Cancelled { .. } | Self::TrackingLimitExceeded { .. } => {
                None
            }
        }
    }
}

#[derive(Eq, PartialEq)]
struct ActiveFingerprint {
    status: String,
    code: String,
    full_code: Option<String>,
}

impl ActiveFingerprint {
    fn from_order(order: &ActiveOrder) -> Self {
        Self {
            status: order.status.clone(),
            code: order.code.expose().to_owned(),
            full_code: order
                .full_code
                .as_ref()
                .map(|value| value.expose().to_owned()),
        }
    }
}

/// Sequential, bounded, in-memory watcher for changes returned by `sms.active`.
pub struct ActiveOrdersWatcher {
    client: Client,
    options: PollOptions,
    max_tracked_orders: usize,
    fingerprints: BTreeMap<OrderId, ActiveFingerprint>,
    pending: VecDeque<ActiveOrder>,
    next_poll_at: Option<Instant>,
    interval: Duration,
    last_observed: Option<ActiveOrder>,
}

impl ActiveOrdersWatcher {
    pub fn new(
        client: Client,
        options: PollOptions,
        max_tracked_orders: usize,
    ) -> Result<Self, PollOptionsError> {
        if max_tracked_orders == 0 {
            return Err(PollOptionsError::new(
                "max_tracked_orders",
                "must be greater than zero",
            ));
        }
        let interval = options.base_interval;
        Ok(Self {
            client,
            options,
            max_tracked_orders,
            fingerprints: BTreeMap::new(),
            pending: VecDeque::new(),
            next_poll_at: None,
            interval,
            last_observed: None,
        })
    }

    pub fn tracked_order_count(&self) -> usize {
        self.fingerprints.len()
    }

    pub fn max_tracked_orders(&self) -> usize {
        self.max_tracked_orders
    }

    /// Returns the next new or changed active order.
    ///
    /// Orders absent from a later active snapshot are removed from the in-memory fingerprint map;
    /// if they reappear, they emit again. This is not durable completion tracking.
    pub async fn next(&mut self) -> Result<ActiveOrder, WatchError> {
        // A queued event never outranks an explicit caller stop condition.
        self.preflight()?;
        if let Some(event) = self.pending.pop_front() {
            return Ok(event);
        }

        loop {
            self.wait_until_next_poll().await?;

            let sms_api = self.client.sms();
            let active = tokio::select! {
                biased;
                _ = self.options.cancellation.cancelled() => {
                    return Err(WatchError::Cancelled {
                        last_observed: self.last_observed.clone().map(Box::new),
                    });
                }
                _ = sleep_until(self.options.deadline) => {
                    return Err(WatchError::Deadline {
                        last_observed: self.last_observed.clone().map(Box::new),
                    });
                }
                result = sms_api.active() => result,
            };

            match active {
                Ok(orders) => self.record_snapshot(orders)?,
                Err(Error::RateLimited { retry_after, .. }) => {
                    self.schedule_next(retry_after);
                    continue;
                }
                Err(error) => return Err(WatchError::Client(error)),
            }

            self.schedule_next(None);
            if let Some(event) = self.pending.pop_front() {
                return Ok(event);
            }
        }
    }

    fn record_snapshot(&mut self, orders: Vec<ActiveOrder>) -> Result<(), WatchError> {
        let unique = orders
            .into_iter()
            .map(|order| (order.order_code.clone(), order))
            .collect::<BTreeMap<_, _>>();
        if unique.len() > self.max_tracked_orders {
            return Err(WatchError::TrackingLimitExceeded {
                limit: self.max_tracked_orders,
                observed: unique.len(),
            });
        }

        self.fingerprints
            .retain(|order_id, _| unique.contains_key(order_id));
        for (order_id, order) in unique {
            let fingerprint = ActiveFingerprint::from_order(&order);
            if self.fingerprints.get(&order_id) != Some(&fingerprint) {
                self.pending.push_back(order.clone());
            }
            self.fingerprints.insert(order_id, fingerprint);
            self.last_observed = Some(order);
        }
        Ok(())
    }

    fn schedule_next(&mut self, retry_after: Option<Duration>) {
        let normal_delay = self.interval;
        self.interval = advance_interval(self.interval, self.options.max_interval);
        let delay = polling_delay(&self.options, normal_delay, retry_after);
        self.next_poll_at = Some(capped_wake_at(delay, self.options.deadline));
    }

    fn preflight(&self) -> Result<(), WatchError> {
        if self.options.cancellation.is_cancelled() {
            return Err(WatchError::Cancelled {
                last_observed: self.last_observed.clone().map(Box::new),
            });
        }
        if Instant::now() >= self.options.deadline {
            return Err(WatchError::Deadline {
                last_observed: self.last_observed.clone().map(Box::new),
            });
        }
        Ok(())
    }

    async fn wait_until_next_poll(&self) -> Result<(), WatchError> {
        self.preflight()?;
        let Some(next_poll_at) = self.next_poll_at else {
            return Ok(());
        };

        tokio::select! {
            biased;
            _ = self.options.cancellation.cancelled() => Err(WatchError::Cancelled {
                last_observed: self.last_observed.clone().map(Box::new),
            }),
            _ = sleep_until(self.options.deadline) => Err(WatchError::Deadline {
                last_observed: self.last_observed.clone().map(Box::new),
            }),
            _ = sleep_until(next_poll_at) => Ok(()),
        }
    }
}

impl fmt::Debug for ActiveOrdersWatcher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActiveOrdersWatcher")
            .field("max_tracked_orders", &self.max_tracked_orders)
            .field("tracked_order_count", &self.fingerprints.len())
            .field("pending_event_count", &self.pending.len())
            .finish_non_exhaustive()
    }
}

fn advance_interval(current: Duration, maximum: Duration) -> Duration {
    current.checked_mul(2).unwrap_or(maximum).min(maximum)
}

fn polling_delay(
    options: &PollOptions,
    normal_delay: Duration,
    retry_after: Option<Duration>,
) -> Duration {
    let floor = retry_after.unwrap_or(Duration::ZERO);
    let base = normal_delay.max(floor);
    let jittered = jitter(base, options.jitter_ratio);
    if retry_after.is_some() {
        jittered.max(floor)
    } else {
        jittered.min(options.max_interval)
    }
}

fn jitter(duration: Duration, ratio: f64) -> Duration {
    if ratio == 0.0 || duration.is_zero() {
        return duration;
    }
    let factor = rand::rng().random_range((1.0 - ratio)..=(1.0 + ratio));
    let nanos = ((duration.as_nanos() as f64) * factor).round();
    duration_from_nanos(nanos.max(0.0) as u128)
}

fn duration_from_nanos(nanos: u128) -> Duration {
    const NANOS_PER_SECOND: u128 = 1_000_000_000;
    let seconds = nanos / NANOS_PER_SECOND;
    if seconds > u64::MAX as u128 {
        return Duration::MAX;
    }
    Duration::new(seconds as u64, (nanos % NANOS_PER_SECOND) as u32)
}

fn capped_wake_at(delay: Duration, deadline: Instant) -> Instant {
    Instant::now()
        .checked_add(delay)
        .unwrap_or(deadline)
        .min(deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> PollOptions {
        PollOptions::new(
            Instant::now() + Duration::from_secs(10),
            CancellationToken::new(),
        )
        .with_intervals(Duration::from_millis(10), Duration::from_millis(100))
        .unwrap()
        .with_jitter_ratio(0.0)
        .unwrap()
    }

    #[test]
    fn validates_intervals_and_jitter() {
        let base = PollOptions::new(
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        );
        assert_eq!(
            base.clone()
                .with_intervals(Duration::ZERO, Duration::from_secs(1))
                .unwrap_err()
                .field(),
            "base_interval"
        );
        assert_eq!(
            base.clone()
                .with_intervals(Duration::from_secs(2), Duration::from_secs(1))
                .unwrap_err()
                .field(),
            "max_interval"
        );
        assert_eq!(
            base.with_jitter_ratio(1.1).unwrap_err().field(),
            "jitter_ratio"
        );
    }

    #[test]
    fn retry_after_is_a_delay_floor() {
        let options = options();
        assert_eq!(
            polling_delay(
                &options,
                Duration::from_millis(10),
                Some(Duration::from_millis(75)),
            ),
            Duration::from_millis(75)
        );
    }

    #[test]
    fn normal_delay_is_capped_after_jitter() {
        let options = options();
        assert_eq!(
            polling_delay(&options, Duration::from_secs(1), None),
            Duration::from_millis(100)
        );
    }
}

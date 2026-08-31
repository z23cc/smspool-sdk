//! PostgreSQL-backed durable order state for the opt-in Axum example.
//!
//! This module belongs to the consuming application, not the core SDK. The provider order ID is
//! encrypted at rest with a fresh XChaCha20 nonce. No phone number, SMS body, API key, raw provider
//! JSON, or absolute balance is persisted.

use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use secrecy::{ExposeSecret, Secret};
use serde::Serialize;
use sha2::{Digest, Sha256};
use smspool::{sms::SmsOrder, OrderId};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use thiserror::Error;

const MIGRATION: &str = include_str!("postgres/migrations/0001_durable_orders.sql");
type EncryptedOrder = (Vec<u8>, [u8; 24], Vec<u8>);

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database operation failed")]
    Database(#[source] sqlx::Error),
    #[error("encryption operation failed")]
    Encryption,
    #[error("stored order could not be decrypted")]
    Decryption,
    #[error("encryption key must be exactly 64 hexadecimal characters")]
    InvalidKey,
    #[error("claim is stale or owned by another worker")]
    StaleClaim,
    #[error("invalid persisted state")]
    InvalidState,
    #[error("application correlation must not be empty")]
    InvalidCorrelation,
    #[error("lease must exceed the worst-case in-flight duration of a provider call")]
    LeaseTooShort { lease_ms: i64, minimum_ms: i64 },
}

#[derive(Clone)]
pub struct OrderStore {
    pool: PgPool,
    key: Secret<[u8; 32]>,
    owner: String,
    lease_ms: i64,
}

impl fmt::Debug for OrderStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OrderStore")
            .field("owner", &self.owner)
            .field("lease_ms", &self.lease_ms)
            .field("encryption_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(transparent)]
pub struct OrderReference(String);

#[allow(dead_code)]
impl OrderReference {
    pub fn parse(value: impl Into<String>) -> Result<Self, StoreError> {
        let value = value.into();
        let bytes = hex::decode(&value).map_err(|_| StoreError::InvalidKey)?;
        if bytes.len() != 16 {
            return Err(StoreError::InvalidKey);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredState {
    Polling,
    Received,
    Terminated,
    Expired,
    ReconcileOnly,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredIntentState {
    Pending,
    ReconcileOnly,
    Resolved,
}

impl StoredIntentState {
    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "pending" => Ok(Self::Pending),
            "reconcile_only" => Ok(Self::ReconcileOnly),
            "resolved" => Ok(Self::Resolved),
            _ => Err(StoreError::InvalidState),
        }
    }
}

impl StoredState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Polling => "polling",
            Self::Received => "received",
            Self::Terminated => "terminated",
            Self::Expired => "expired",
            Self::ReconcileOnly => "reconcile_only",
            Self::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "polling" => Ok(Self::Polling),
            "received" => Ok(Self::Received),
            "terminated" => Ok(Self::Terminated),
            "expired" => Ok(Self::Expired),
            "reconcile_only" => Ok(Self::ReconcileOnly),
            "failed" => Ok(Self::Failed),
            _ => Err(StoreError::InvalidState),
        }
    }
}

/// An order claimed by a single worker. Debug omits the decrypted provider identifier.
pub struct ClaimedOrder {
    row_id: i64,
    reference: OrderReference,
    order_id: OrderId,
    state: StoredState,
    deadline_ms: i64,
    version: i64,
    owner: String,
}

impl fmt::Debug for ClaimedOrder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaimedOrder")
            .field("row_id", &self.row_id)
            .field("reference", &self.reference)
            .field("version", &self.version)
            .field("owner", &self.owner)
            .field("state", &self.state)
            .field("order_id", &"[REDACTED]")
            .field("deadline_ms", &self.deadline_ms)
            .finish()
    }
}

#[allow(dead_code)]
impl ClaimedOrder {
    pub fn row_id(&self) -> i64 {
        self.row_id
    }
    pub fn reference(&self) -> &OrderReference {
        &self.reference
    }
    pub fn order_id(&self) -> &OrderId {
        &self.order_id
    }
    pub fn version(&self) -> i64 {
        self.version
    }
    pub fn state(&self) -> StoredState {
        self.state
    }
    pub fn deadline_ms(&self) -> i64 {
        self.deadline_ms
    }
}

#[allow(dead_code)]
impl OrderStore {
    /// `min_lease` must be the consuming client's [`smspool::Client::max_in_flight_duration`].
    ///
    /// A claim is only safe if it outlives the longest provider call made while holding it.
    pub async fn connect(
        database_url: &str,
        key_hex: &str,
        owner: impl Into<String>,
        lease: std::time::Duration,
        min_lease: std::time::Duration,
    ) -> Result<Self, StoreError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await
            .map_err(StoreError::Database)?;
        Self::from_pool(pool, key_hex, owner, lease, min_lease)
    }

    /// Rejects a lease that does not strictly exceed `min_lease`.
    ///
    /// This is enforced at construction so that retuning a client timeout cannot silently
    /// reintroduce the window where a lease expires under an in-flight request.
    pub fn from_pool(
        pool: PgPool,
        key_hex: &str,
        owner: impl Into<String>,
        lease: std::time::Duration,
        min_lease: std::time::Duration,
    ) -> Result<Self, StoreError> {
        let key_vec = hex::decode(key_hex).map_err(|_| StoreError::InvalidKey)?;
        let key: [u8; 32] = key_vec.try_into().map_err(|_| StoreError::InvalidKey)?;
        let lease_ms = i64::try_from(lease.as_millis()).map_err(|_| StoreError::InvalidKey)?;
        if lease_ms <= 0 {
            return Err(StoreError::InvalidKey);
        }
        let minimum_ms =
            i64::try_from(min_lease.as_millis()).map_err(|_| StoreError::InvalidKey)?;
        if lease_ms <= minimum_ms {
            return Err(StoreError::LeaseTooShort {
                lease_ms,
                minimum_ms,
            });
        }
        Ok(Self {
            pool,
            key: Secret::new(key),
            owner: owner.into(),
            lease_ms,
        })
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        sqlx::raw_sql(MIGRATION)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(StoreError::Database)
    }

    /// Record an application intent before issuing a paid purchase request.
    ///
    /// Only a keyed, irreversible fingerprint is stored. If the provider response is ambiguous,
    /// transition this intent to `reconcile_only` instead of replaying the purchase.
    pub async fn record_purchase_intent(
        &self,
        application_correlation: &str,
    ) -> Result<OrderReference, StoreError> {
        if application_correlation.trim().is_empty() {
            return Err(StoreError::InvalidCorrelation);
        }
        let fingerprint = self.fingerprint(application_correlation);
        let now = now_ms();
        sqlx::query(
            "INSERT INTO smspool_example_mutation_intents
             (correlation_fingerprint, operation, state, created_at_ms, updated_at_ms)
             VALUES ($1, 'purchase_sms', 'pending', $2, $2)
             ON CONFLICT (correlation_fingerprint) DO NOTHING",
        )
        .bind(&fingerprint)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        Ok(OrderReference(hex::encode(fingerprint)))
    }

    /// Mark a pre-recorded purchase intent as resolved after the provider order is durably stored.
    pub async fn resolve_purchase_intent(
        &self,
        reference: &OrderReference,
    ) -> Result<(), StoreError> {
        let result = sqlx::query(
            "UPDATE smspool_example_mutation_intents
             SET state='resolved', updated_at_ms=$1
             WHERE correlation_fingerprint=$2 AND operation='purchase_sms' AND state IN ('pending', 'reconcile_only')",
        )
        .bind(now_ms())
        .bind(hex::decode(reference.as_str()).map_err(|_| StoreError::InvalidKey)?)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::StaleClaim);
        }
        Ok(())
    }

    /// Mark a pre-recorded purchase intent as requiring read-only reconciliation.
    pub async fn mark_purchase_intent_reconcile_only(
        &self,
        reference: &OrderReference,
    ) -> Result<(), StoreError> {
        let result = sqlx::query(
            "UPDATE smspool_example_mutation_intents
             SET state='reconcile_only', updated_at_ms=$1
             WHERE correlation_fingerprint=$2 AND operation='purchase_sms' AND state='pending'",
        )
        .bind(now_ms())
        .bind(hex::decode(reference.as_str()).map_err(|_| StoreError::InvalidKey)?)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::StaleClaim);
        }
        Ok(())
    }

    pub async fn purchase_intent_status(
        &self,
        reference: &OrderReference,
    ) -> Result<Option<StoredIntentState>, StoreError> {
        let fingerprint = hex::decode(reference.as_str()).map_err(|_| StoreError::InvalidKey)?;
        let row = sqlx::query(
            "SELECT state FROM smspool_example_mutation_intents WHERE correlation_fingerprint=$1",
        )
        .bind(fingerprint)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        row.map(|row| {
            row.try_get::<String, _>("state")
                .map_err(StoreError::Database)
                .and_then(|state| StoredIntentState::parse(&state))
        })
        .transpose()
    }

    /// Persist a decoded successful purchase before spawning a polling task.
    pub async fn record_purchase(
        &self,
        order: &SmsOrder,
        deadline_ms: i64,
    ) -> Result<OrderReference, StoreError> {
        let (fingerprint, nonce, ciphertext) = self.encrypt_order_id(&order.order_id)?;
        let now = now_ms();
        sqlx::query(
            "INSERT INTO smspool_example_orders
             (correlation_fingerprint, order_id_nonce, order_id_ciphertext, state,
              deadline_ms, next_poll_ms, poll_attempts, created_at_ms, updated_at_ms)
             VALUES ($1, $2, $3, 'polling', $4, $5, 0, $6, $6)
             ON CONFLICT (correlation_fingerprint) DO NOTHING",
        )
        .bind(&fingerprint)
        .bind(nonce.as_slice())
        .bind(&ciphertext)
        .bind(deadline_ms)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        Ok(OrderReference(hex::encode(fingerprint)))
    }

    /// Atomically persist a successful purchase, bind it to a pre-recorded intent, and resolve
    /// that intent. Repeating the same call is idempotent; a different order for one intent is
    /// rejected instead of creating a second paid record.
    pub async fn record_purchase_for_intent(
        &self,
        intent: &OrderReference,
        order: &SmsOrder,
        deadline_ms: i64,
    ) -> Result<OrderReference, StoreError> {
        let intent_fingerprint = reference_bytes(intent)?;
        let (fingerprint, nonce, ciphertext) = self.encrypt_order_id(&order.order_id)?;
        let now = now_ms();
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;

        sqlx::query(
            "INSERT INTO smspool_example_orders
             (correlation_fingerprint, intent_fingerprint, order_id_nonce, order_id_ciphertext,
              state, deadline_ms, next_poll_ms, poll_attempts, created_at_ms, updated_at_ms)
             VALUES ($1, $2, $3, $4, 'polling', $5, $6, 0, $7, $7)
             ON CONFLICT (correlation_fingerprint) DO NOTHING",
        )
        .bind(&fingerprint)
        .bind(&intent_fingerprint)
        .bind(nonce.as_slice())
        .bind(&ciphertext)
        .bind(deadline_ms)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::Database)?;

        sqlx::query(
            "UPDATE smspool_example_orders
             SET intent_fingerprint=$1, updated_at_ms=$2
             WHERE correlation_fingerprint=$3 AND intent_fingerprint IS NULL",
        )
        .bind(&intent_fingerprint)
        .bind(now)
        .bind(&fingerprint)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::Database)?;

        let order_row = sqlx::query(
            "SELECT intent_fingerprint FROM smspool_example_orders
             WHERE correlation_fingerprint=$1",
        )
        .bind(&fingerprint)
        .fetch_optional(&mut *tx)
        .await
        .map_err(StoreError::Database)?
        .ok_or(StoreError::StaleClaim)?;
        let linked_intent: Option<Vec<u8>> = order_row
            .try_get("intent_fingerprint")
            .map_err(StoreError::Database)?;
        if linked_intent.as_deref() != Some(intent_fingerprint.as_slice()) {
            return Err(StoreError::StaleClaim);
        }

        let resolved = sqlx::query(
            "UPDATE smspool_example_mutation_intents
             SET state='resolved', updated_at_ms=$1
             WHERE correlation_fingerprint=$2 AND operation='purchase_sms'
               AND state IN ('pending', 'reconcile_only')",
        )
        .bind(now)
        .bind(&intent_fingerprint)
        .execute(&mut *tx)
        .await
        .map_err(StoreError::Database)?;
        if resolved.rows_affected() == 0 {
            let state = sqlx::query(
                "SELECT state FROM smspool_example_mutation_intents
                 WHERE correlation_fingerprint=$1 AND operation='purchase_sms'",
            )
            .bind(&intent_fingerprint)
            .fetch_optional(&mut *tx)
            .await
            .map_err(StoreError::Database)?
            .map(|row| row.try_get::<String, _>("state"))
            .transpose()
            .map_err(StoreError::Database)?;
            if state.as_deref() != Some("resolved") {
                return Err(StoreError::StaleClaim);
            }
        }

        tx.commit().await.map_err(StoreError::Database)?;
        Ok(OrderReference(hex::encode(fingerprint)))
    }

    /// Record a previously persisted order as reconcile-only after an ambiguous mutation.
    pub async fn mark_reconcile_only(&self, reference: &OrderReference) -> Result<(), StoreError> {
        let result = sqlx::query(
            "UPDATE smspool_example_orders SET state='reconcile_only', updated_at_ms=$1
             WHERE correlation_fingerprint=$2 AND state IN ('polling', 'reconcile_only')",
        )
        .bind(now_ms())
        .bind(hex::decode(reference.as_str()).map_err(|_| StoreError::InvalidKey)?)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::StaleClaim);
        }
        Ok(())
    }

    /// Claim due rows atomically. `FOR UPDATE SKIP LOCKED` prevents duplicate workers.
    pub async fn claim_due(&self, limit: u32) -> Result<Vec<ClaimedOrder>, StoreError> {
        self.claim_due_at(now_ms(), limit).await
    }

    pub async fn claim_due_at(
        &self,
        now: i64,
        limit: u32,
    ) -> Result<Vec<ClaimedOrder>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let lease_until = now.saturating_add(self.lease_ms);
        let mut tx = self.pool.begin().await.map_err(StoreError::Database)?;
        let rows = sqlx::query(
            "SELECT id, correlation_fingerprint, order_id_nonce, order_id_ciphertext, state, deadline_ms, version
             FROM smspool_example_orders
             WHERE state IN ('polling', 'reconcile_only')
               AND next_poll_ms <= $1
               AND (lease_until_ms IS NULL OR lease_until_ms < $1)
             ORDER BY id
             FOR UPDATE SKIP LOCKED
             LIMIT $2",
        )
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(StoreError::Database)?;

        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            let row_id: i64 = row.try_get("id").map_err(StoreError::Database)?;
            let fingerprint: Vec<u8> = row
                .try_get("correlation_fingerprint")
                .map_err(StoreError::Database)?;
            let nonce: Vec<u8> = row
                .try_get("order_id_nonce")
                .map_err(StoreError::Database)?;
            let ciphertext: Vec<u8> = row
                .try_get("order_id_ciphertext")
                .map_err(StoreError::Database)?;
            let state: String = row.try_get("state").map_err(StoreError::Database)?;
            let state = StoredState::parse(&state)?;
            let deadline_ms: i64 = row.try_get("deadline_ms").map_err(StoreError::Database)?;
            let version: i64 = row.try_get("version").map_err(StoreError::Database)?;
            let order_id = self.decrypt_order_id(&nonce, &ciphertext)?;
            let new_version = version.saturating_add(1);
            let updated = sqlx::query(
                "UPDATE smspool_example_orders
                 SET lease_owner=$1, lease_until_ms=$2, version=$3, updated_at_ms=$4
                 WHERE id=$5 AND version=$6 AND (lease_until_ms IS NULL OR lease_until_ms < $7)",
            )
            .bind(&self.owner)
            .bind(lease_until)
            .bind(new_version)
            .bind(now)
            .bind(row_id)
            .bind(version)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(StoreError::Database)?;
            if updated.rows_affected() != 1 {
                return Err(StoreError::StaleClaim);
            }
            claimed.push(ClaimedOrder {
                row_id,
                reference: OrderReference(hex::encode(fingerprint)),
                order_id,
                state,
                deadline_ms,
                version: new_version,
                owner: self.owner.clone(),
            });
        }
        tx.commit().await.map_err(StoreError::Database)?;
        Ok(claimed)
    }

    pub async fn record_pending(
        &self,
        claim: &mut ClaimedOrder,
        next_poll_ms: i64,
    ) -> Result<(), StoreError> {
        self.update_claim(claim, StoredState::Polling, next_poll_ms, true)
            .await
    }

    pub async fn record_terminal(
        &self,
        claim: &mut ClaimedOrder,
        state: StoredState,
    ) -> Result<(), StoreError> {
        if !matches!(state, StoredState::Received | StoredState::Terminated) {
            return Err(StoreError::InvalidState);
        }
        self.update_claim(claim, state, now_ms(), false).await
    }

    pub async fn record_expired(&self, claim: &mut ClaimedOrder) -> Result<(), StoreError> {
        self.update_claim(claim, StoredState::Expired, now_ms(), false)
            .await
    }

    /// Release a claim after a transient read failure while keeping the row claimable.
    pub async fn release_for_retry(
        &self,
        claim: &mut ClaimedOrder,
        next_poll_ms: i64,
    ) -> Result<(), StoreError> {
        let state = match claim.state {
            StoredState::Polling | StoredState::ReconcileOnly => claim.state,
            StoredState::Received
            | StoredState::Terminated
            | StoredState::Expired
            | StoredState::Failed => return Err(StoreError::InvalidState),
        };
        self.update_claim(claim, state, next_poll_ms, true).await
    }

    /// Mark a claim failed when the consuming application has exhausted its retry policy.
    pub async fn release_or_fail(
        &self,
        claim: &mut ClaimedOrder,
        next_poll_ms: i64,
    ) -> Result<(), StoreError> {
        self.update_claim(claim, StoredState::Failed, next_poll_ms, false)
            .await
    }

    pub async fn record_reconcile_only(
        &self,
        claim: &mut ClaimedOrder,
        next_poll_ms: i64,
    ) -> Result<(), StoreError> {
        self.update_claim(claim, StoredState::ReconcileOnly, next_poll_ms, false)
            .await
    }

    pub async fn status(
        &self,
        reference: &OrderReference,
    ) -> Result<Option<StoredState>, StoreError> {
        let fingerprint = hex::decode(reference.as_str()).map_err(|_| StoreError::InvalidKey)?;
        let row = sqlx::query(
            "SELECT state FROM smspool_example_orders WHERE correlation_fingerprint=$1",
        )
        .bind(fingerprint)
        .fetch_optional(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        row.map(|row| {
            row.try_get::<String, _>("state")
                .map_err(StoreError::Database)
                .and_then(|state| StoredState::parse(&state))
        })
        .transpose()
    }

    /// Test-only helper used by the opt-in restart exercise.
    pub async fn expire_lease_for_test(&self, row_id: i64) -> Result<(), StoreError> {
        sqlx::query("UPDATE smspool_example_orders SET lease_until_ms=0 WHERE id=$1")
            .bind(row_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(StoreError::Database)
    }

    pub async fn table_text_for_test(&self) -> Result<Vec<String>, StoreError> {
        let rows = sqlx::query("SELECT encode(order_id_ciphertext, 'hex') AS ciphertext, state FROM smspool_example_orders")
            .fetch_all(&self.pool)
            .await
            .map_err(StoreError::Database)?;
        rows.into_iter()
            .map(|row| {
                let ciphertext: String = row.try_get("ciphertext").map_err(StoreError::Database)?;
                let state: String = row.try_get("state").map_err(StoreError::Database)?;
                Ok(format!("{ciphertext}:{state}"))
            })
            .collect()
    }

    async fn update_claim(
        &self,
        claim: &mut ClaimedOrder,
        state: StoredState,
        next_poll_ms: i64,
        increment_attempt: bool,
    ) -> Result<(), StoreError> {
        let new_version = claim.version.saturating_add(1);
        let result = sqlx::query(
            "UPDATE smspool_example_orders
             SET state=$1, next_poll_ms=$2, poll_attempts=poll_attempts + $3,
                 lease_owner=NULL, lease_until_ms=NULL, version=$4, updated_at_ms=$5
             WHERE id=$6 AND lease_owner=$7 AND version=$8",
        )
        .bind(state.as_str())
        .bind(next_poll_ms)
        .bind(if increment_attempt { 1_i64 } else { 0_i64 })
        .bind(new_version)
        .bind(now_ms())
        .bind(claim.row_id)
        .bind(&claim.owner)
        .bind(claim.version)
        .execute(&self.pool)
        .await
        .map_err(StoreError::Database)?;
        if result.rows_affected() != 1 {
            return Err(StoreError::StaleClaim);
        }
        claim.version = new_version;
        claim.state = state;
        Ok(())
    }

    fn fingerprint(&self, value: &str) -> Vec<u8> {
        let mut digest = Sha256::new();
        digest.update(self.key.expose_secret());
        digest.update(value.as_bytes());
        digest.finalize()[..16].to_vec()
    }

    fn encrypt_order_id(&self, order_id: &OrderId) -> Result<EncryptedOrder, StoreError> {
        let mut nonce = [0_u8; 24];
        rand::rng().fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.expose_secret())
            .map_err(|_| StoreError::Encryption)?;
        let ciphertext = cipher
            .encrypt(XNonce::from_slice(&nonce), order_id.as_str().as_bytes())
            .map_err(|_| StoreError::Encryption)?;
        let fingerprint = self.fingerprint(order_id.as_str());
        Ok((fingerprint, nonce, ciphertext))
    }

    fn decrypt_order_id(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<OrderId, StoreError> {
        let nonce: [u8; 24] = nonce.try_into().map_err(|_| StoreError::Decryption)?;
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.expose_secret())
            .map_err(|_| StoreError::Decryption)?;
        let plaintext = cipher
            .decrypt(XNonce::from_slice(&nonce), ciphertext)
            .map_err(|_| StoreError::Decryption)?;
        let value = String::from_utf8(plaintext).map_err(|_| StoreError::Decryption)?;
        OrderId::new(value).map_err(|_| StoreError::Decryption)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

/// The integration point a consuming purchase coordinator should call synchronously.
#[allow(dead_code)]
pub async fn record_successful_purchase(
    store: &OrderStore,
    order: &SmsOrder,
    deadline_ms: i64,
) -> Result<OrderReference, StoreError> {
    store.record_purchase(order, deadline_ms).await
}

/// Atomically persist a purchase and resolve the caller's pre-recorded intent.
#[allow(dead_code)]
pub async fn record_successful_purchase_for_intent(
    store: &OrderStore,
    intent: &OrderReference,
    order: &SmsOrder,
    deadline_ms: i64,
) -> Result<OrderReference, StoreError> {
    store
        .record_purchase_for_intent(intent, order, deadline_ms)
        .await
}

fn reference_bytes(reference: &OrderReference) -> Result<Vec<u8>, StoreError> {
    let bytes = hex::decode(reference.as_str()).map_err(|_| StoreError::InvalidKey)?;
    if bytes.len() != 16 {
        return Err(StoreError::InvalidKey);
    }
    Ok(bytes)
}

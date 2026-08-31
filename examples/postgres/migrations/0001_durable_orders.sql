CREATE TABLE IF NOT EXISTS smspool_example_orders (
    id BIGSERIAL PRIMARY KEY,
    correlation_fingerprint BYTEA NOT NULL UNIQUE,
    intent_fingerprint BYTEA CHECK (intent_fingerprint IS NULL OR octet_length(intent_fingerprint) = 16),
    order_id_nonce BYTEA NOT NULL CHECK (octet_length(order_id_nonce) = 24),
    order_id_ciphertext BYTEA NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('polling', 'received', 'terminated', 'expired', 'reconcile_only', 'failed')),
    deadline_ms BIGINT NOT NULL,
    next_poll_ms BIGINT NOT NULL,
    poll_attempts BIGINT NOT NULL DEFAULT 0,
    lease_owner TEXT,
    lease_until_ms BIGINT,
    version BIGINT NOT NULL DEFAULT 0,
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL
);

-- Keep the example migration restartable when an older local copy already created the table.
ALTER TABLE smspool_example_orders
    ADD COLUMN IF NOT EXISTS intent_fingerprint BYTEA;

CREATE INDEX IF NOT EXISTS smspool_example_orders_due_idx
    ON smspool_example_orders (state, next_poll_ms, lease_until_ms);

CREATE UNIQUE INDEX IF NOT EXISTS smspool_example_orders_intent_idx
    ON smspool_example_orders (intent_fingerprint)
    WHERE intent_fingerprint IS NOT NULL;

-- This ledger stores only an application correlation fingerprint. It allows an application to
-- record a paid-mutation intent before sending the provider request, without inventing an order ID
-- when the response is ambiguous.

CREATE TABLE IF NOT EXISTS smspool_example_mutation_intents (
    id BIGSERIAL PRIMARY KEY,
    correlation_fingerprint BYTEA NOT NULL UNIQUE CHECK (octet_length(correlation_fingerprint) = 16),
    operation TEXT NOT NULL CHECK (operation IN ('purchase_sms')),
    state TEXT NOT NULL CHECK (state IN ('pending', 'reconcile_only', 'resolved')),
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL
);

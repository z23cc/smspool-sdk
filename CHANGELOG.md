# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Async `Client` covering all 60 SMSPool operations. Catalog, Pricing, and core SMS are the
  stable surface; the remaining 46 operations are isolated behind `Client::experimental()`.
- `wait_for_sms`, `wait_for_code_with`, and `ActiveOrdersWatcher` polling workflows.
- `cancel_with_reconciliation`: bounded cancellation with exact time-lock retry and read-only
  reconciliation. Never replays a mutation on an ambiguous outcome.
- `Error::OutcomeUnknown` for non-idempotent calls whose result cannot be determined.
- Opt-in client-side QPS pacing (`max_requests_per_second`) shared across `Client` clones.
- `Client::max_in_flight_duration()` so callers can size locks and leases correctly.
- `SmsCheck::status()`, `status_code()`, and `is_terminal()`.
- Axum + PostgreSQL example with durable order state, claim leases, and restart recovery.
- Acceptance gate tooling (`scripts/acceptance.sh`) with `foundation`, `sdk`, and
  `production` profiles.

### Changed

- **MSRV raised from 1.82 to 1.85.** Several dependencies moved to edition 2024, which
  Cargo only supports from 1.85. The `postgres-example` dev feature needs a newer
  toolchain still, because sqlx 0.9 declares `rust-version = 1.94`; that constraint does
  not apply to the published library.
- Dependencies moved to their current major versions: reqwest 0.13, rand 0.10, sqlx 0.9,
  sha2 0.11, chacha20poly1305 0.11, plus routine minor and patch updates. This required
  migrations: reqwest renamed its rustls feature and split out `form`/`query`; sqlx
  renamed its runtime/TLS features and now requires dynamic SQL to be wrapped in
  `AssertSqlSafe`; rand moved `random_range` to `RngExt` and dropped `RngCore` from the
  root; chacha20poly1305 deprecated `Array::from_slice`.
- Licensed under Apache-2.0 only; the previous `MIT OR Apache-2.0` dual licence and
  `LICENSE-MIT` are removed. Copyright attribution now lives in `NOTICE`.

### Security

- API keys, phone numbers, SMS bodies, eSIM credentials, and arbitrary JSON fallbacks are
  redacted in `Debug` and tracing output by default.
- Automatic redirects are disabled so a cross-origin redirect cannot leak the form-field API key.
- HTTP mock support is restricted to loopback and bypasses environment proxies.

### Known limitations

- Not published. `documentation` is intentionally unset so crates.io falls back to docs.rs.
  See `CONTRIBUTING.md` for the release checklist.
- `sms.all_stock` and unfiltered `pricing.all` fail closed: their live responses measured
  ~16 MiB and ~17.2 MiB respectively, far beyond any safe buffered limit.
- The 429 / `Retry-After` path is covered by mocks only. It could not be induced live: 200
  requests at ~92 rps produced no rate limiting and no rate-limit headers.
- The end-to-end receive sequence (purchase -> real inbound SMS -> `wait_for_sms`) has not been
  exercised. Decoding of received messages was verified against real completed orders.
- `Error::OutcomeUnknown` has never been observed live.
- Experimental operations have fixture coverage only.
- The public API is not documented to `missing_docs` standard (~737 items).

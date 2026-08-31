# Contributing

## Prerequisites

- Rust stable and 1.82.0 (the MSRV)
- Python 3.11+ for the acceptance and contract tooling
- Docker, only for the opt-in PostgreSQL recovery test

## The one command that matters

```bash
./scripts/acceptance.sh sdk
```

This is the gate CI runs. It covers formatting, `clippy -D warnings`, unit and integration
tests, decoding of all 103 Postman fixtures, the request-wire contract, and the redaction suite.
It is fully offline and never reads `SMSPOOL_API_KEY`.

Other profiles:

| Profile | Scope |
|---|---|
| `foundation` | Docs, tooling tests, contract fingerprint |
| `sdk` | Everything above plus the full Rust suite (CI gate) |
| `production` | Adds manual gates that require live evidence and an operator sign-off |

`production` is expected to report blocked gates. `LIVE-001`, `OPS-001`, and `PILOT-001` cannot
be satisfied by this repository alone.

## Changing the provider contract

`postman.json` is the source of truth for the wire contract. After editing it:

```bash
python3 scripts/postman_contract.py regenerate
./scripts/acceptance.sh foundation
```

This refreshes `contracts/postman-baseline.json` and the generated endpoint matrix. The
fixture test asserts that every field the SDK sends is declared in the baseline, and that every
field the collection marks enabled is actually sent.

## Testing rules

- **Never put real customer data in fixtures.** No real verification codes, SMS bodies, phone
  numbers, or order identifiers. Use synthetic values with the same structure.
- Tests that spend money or require credentials must be `#[ignore]`d *and* require an explicit
  opt-in environment variable. See `tests/live_receive.rs`.
- A test asserting a fix should be shown to fail against the unfixed code before it is trusted.

## Opt-in tests

```bash
# Durable recovery. Needs PostgreSQL; makes no provider requests.
DATABASE_URL=postgres://... SMSPOOL_ORDER_KEY=<64 hex chars> \
  cargo test --features postgres-example --test postgres_recovery -- --ignored

# Live SMS receive capture. Spends money that is NOT refundable once an SMS lands.
# Read the module docs before running.
cargo test --test live_receive -- --ignored --nocapture
```

## Release checklist

The crate is not published. Before the first release:

1. Set `repository` and `documentation` in `Cargo.toml`.
2. Replace `<COPYRIGHT HOLDER>` in `LICENSE-MIT`.
3. Move the `Unreleased` section of `CHANGELOG.md` under a version heading.
4. Confirm `cargo package --locked` succeeds and review `cargo package --list`.
5. Run `./scripts/acceptance.sh sdk`.
6. Consider `cargo-semver-checks` once a baseline version exists.

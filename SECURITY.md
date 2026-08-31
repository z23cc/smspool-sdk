# Security

## Reporting

Report suspected vulnerabilities privately to the repository owner rather than opening a public
issue. Include the affected version and a reproduction if you have one.

## What this SDK protects by default

- **Credentials.** The API key is held in a secret wrapper and never appears in `Debug`,
  `Display`, tracing output, or error messages.
- **Customer data.** Phone numbers, SMS bodies and codes, eSIM credentials, activation tokens,
  and arbitrary JSON fallbacks are redacted in `Debug` and tracing output. Redaction is verified
  against real-world multi-byte content, not only ASCII placeholders.
- **Credential exfiltration via redirects.** Automatic redirect following is disabled. Some
  operations send the API key as a form field, so a cross-origin redirect would otherwise
  forward it to a third party. There is a regression test for this.
- **Unbounded responses.** Every response is size-limited. Two operations whose live responses
  exceed any safe buffer fail closed rather than attempting to read them.
- **Test isolation.** HTTP mocking is restricted to loopback addresses and bypasses environment
  proxies, so a stray `HTTP_PROXY` cannot capture test traffic.

## What it does not protect

- **Log output you write yourself.** `RedactedValue::expose()` returns plaintext by design.
  Anything you log after calling it is your responsibility.
- **Key lifecycle.** There is no key versioning, rotation, or revocation propagation. Keep keys
  in a secret manager. Rotating an SMSPool key invalidates the previous one immediately.
- **At-rest encryption of order state.** The PostgreSQL example encrypts provider order
  identifiers, but your own persistence layer is yours to secure.

## Operational rules

- Never paste a live API key into an issue, pull request, commit, chat transcript, or log.
  Treat any key that has appeared in one as compromised and rotate it.
- Never commit real verification codes, SMS bodies, phone numbers, or order identifiers, even
  expired ones. Fixtures must use synthetic data.
- The client-side rate limiter is **opt-in and per-process**. It is not enabled by default and
  does not coordinate across instances.

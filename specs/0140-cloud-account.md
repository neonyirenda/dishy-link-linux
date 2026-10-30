---
status: active
---

# 0140 — Starlink cloud account

## Purpose

Port the account-backed plan, billing usage, device telemetry, and router
fallback reads used by the Electron app. Cloud requests stay isolated from the
LAN recorder and carry credentials only inside `dishy-cloud`.

## Requirements

- **SPEC-CLOUD-001** — Cloud reads use the official Starlink auth and API
  endpoints, refresh the short-lived `Access.V1` cookie at most once per minute,
  and share an in-flight refresh between concurrent reads.
- **SPEC-CLOUD-002** — A rejected session is reported as `not_connected`,
  distinct from an upstream/network failure; a single expired-token response
  forces one refresh and one retry.
- **SPEC-CLOUD-003** — Account and usage reads resolve the account reference and
  service line from the authenticated account, then return the account identity,
  service-line detail, device telemetry, and annotated usage expected by the UI.
- **SPEC-CLOUD-004** — A session cookie is stored by the host in the OS secret
  store. It is never returned to GTK widgets, written to logs, or sent to a
  renderer-controlled URL.
- **SPEC-CLOUD-005** — Cloud data requests are on-demand and do not depend on
  local dish/router reachability.
- **SPEC-CLOUD-006** — The account page exposes signed-out, loading, ready, and
  retryable-error states, with sign-in, refresh, and disconnect actions routed
  through the trusted GTK host.
- **SPEC-CLOUD-007** — Account and billing usage are fetched when the account
  page is opened or explicitly refreshed; startup checks session presence only.
- **SPEC-CLOUD-008** — The isolated sign-in window has a visible top-right
  Close action that returns to the app. Detecting the Starlink SSO cookie
  submits a candidate session; the nested window closes only after the host
  verifies and stores it successfully.
- **SPEC-CLOUD-009** — Sign-in verification tolerates the Starlink SSO cookie
  arriving before its associated access token. Temporary authentication
  rejection is retried up to four times before the stored session is cleared.
- **SPEC-CLOUD-010** — The candidate session is every cookie in the login
  jar for `starlink.com` and its subdomains, whatever its path, as Electron's
  `cookies.get({ domain: "starlink.com" })` reads it. Cookies scoped to other
  hosts or paths, such as the API's, are part of the session; reading only
  the cookies a single page URL would send drops them, and the account API
  rejects what remains. Sign-in logs each step by cookie name only.
- **SPEC-CLOUD-011** — The login jar is resubmitted as each cookie lands.
  Only the newest pending candidate is verified, and older snapshots queued
  behind a verification are dropped. Otherwise an early snapshot without the
  access tokens fails and clears the store while the complete one waits.
  Requests other than sign-in keep their order.

## Acceptance criteria

- `dishy-cloud` contract tests use a fake HTTP service for refresh, account,
  usage, authentication expiry, retry, and upstream failure behavior.
- GTK tests distinguish loading, signed-out, ready, and retryable-error states.
- The cloud crate has no dependency on GTK or the recorder; the session store
  is supplied by the host and is backed by the Linux secret service.
- `cargo fmt --check`, `cargo clippy -- -D warnings`, and `spec_traceability`
  pass.
- The GTK account page renders account/service-plan and newest billing-cycle
  usage without receiving a cookie; the login view is isolated and ephemeral.
- The login chrome's close action returns to the parent window. Successful SSO
  cookie changes are observed even when the account page does not navigate, and
  host verification is what completes sign-in and returns to the account view.

## Deferred

- Router subnet and factory-reset interactions are covered in 0142. The mesh
  rename form remains deferred there.
- A cloud fallback for router roster and config reads is a separate network
  requirement.

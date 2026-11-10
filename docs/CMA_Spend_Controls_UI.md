# Admin Spend controls

Part of #1489. Backend policy and enforcement boundaries are defined in
[CMA_Model_Budget_Controls.md](CMA_Model_Budget_Controls.md).

## Administration

Operations → Platform settings requires ConfigurePlatform. Spend controls edits
use the authenticated settings API, the displayed revision and the existing
full-settings PUT. The SDK retains the additive model_budget section and unrelated
server fields. Backend authorization and revision checks remain authoritative.

The supported section includes enabled, the platform period policy, a default
owner policy and existing explicit owner overrides. Disabled policies retain their
configuration. Existing reservations and stop decisions remain effective. Empty
policies do not inherit frontend amounts, percentages, periods or timezones.

The UI edits platform and default-owner policies. Explicit owner overrides are
counted and retained unchanged; they replace the default for those owners. Owners
without either policy remain outside the enabled scope. Accounting values are
exact integers in the API's accounting unit. Currency/scale metadata is not yet
available, so the UI does not label these values as money or infer conversion.
An older or unsupported API section is preserved and shown as unavailable.

Platform policy fields include period cap, explicit RFC3339 start/end, warning
and stop percentages, protected reserve and lag allowance. Default-owner fields
include daily/turn caps, model-call count, IANA day-boundary timezone, input/output
token valuations, unobserved allowance and usage timeout. Client checks reject
precision loss and invalid bounds/relationships; the server validates its own
bundled timezone database and outstanding accounting constraints.

Configuration availability does not prove strict provider-spend enforcement.
Platform threshold actions and request-level/model-call enforcement depend on
the integrations described by the backend contract. General activation requires
the release owner's direction and configured policy supplied privately.

## Revision recovery and audit

A conflict or uncertain write retains the draft, reads current settings and blocks
another save. Load current values discards the draft deliberately. Review retained
draft applies only edited fields to the current revision and retains concurrent
owner-override/unrelated changes. Neither action automatically writes. If the
current server no longer supports this section, retained spend edits cannot be
reapplied to it. A policy removed concurrently requires its unedited values again.

ReadActivityLog is independently required for last-change lookup. The UI reads
one bounded recent page and matches a succeeded platform-settings operation to
its exact revision. Missing, unavailable or denied details remain explicit; a
stale audit row is not attributed to the current revision. The audit describes
a settings operation, not which budget field changed. Raw actor identifiers and
private audit detail are not displayed in place of a missing name.

## Local verification and release handoff

Unit tests cover validation, exact integers, settings preservation, disablement,
conflict and uncertain-write review, server failures, capability gating and audit.
`npm run e2e:typecheck` checks browser entrypoints. After `npm run build`, run:

```sh
npx playwright test --config e2e/spend-controls.config.ts
```

The loopback harness runs Chromium, Firefox and WebKit against the compiled web
bundle using synthetic session/settings responses. It exercises saves, rejection,
conflict review and explicit disabling without real accounts or backend writes.
Local fixtures do not establish production policy enforcement or live API proof.

The release owner verifies the exact web artifact and backend settings contract
in the approved train. Production UI inspection must use an authorized admin
account; never enable a platform policy or alter allowances merely to test a UI.
A separately authorized test-owner policy can verify backend behavior. Record
artifact/source revision, settings revision and aggregate results only, excluding
identities, credentials and configured policy values. This UI PR does not close
the strict provider-spend acceptance requirement or activate production controls.

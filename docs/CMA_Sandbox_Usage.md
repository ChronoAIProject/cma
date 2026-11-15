# Sandbox usage

CMA records allocation usage at the MongoDB persistence boundary for Sandbox
lifecycle events. The same transaction commits the Sandbox transition, retained
usage state, and the closed usage interval. Runtime and home attachments changed
before a status event are also metered in the aggregate's transaction.

The public API is independent of Agent token usage. Settings → Usage shows the
authenticated owner's Sandboxes. Operations → Usage requires
`cma:admin:sandboxes` and shows every owner's Sandboxes, with an optional owner
filter. Both views use the public API and TypeScript SDK.

## Measurements

All intervals are half-open, `[started_at, ended_at)`, at the existing lifecycle
clock's one-second resolution. Durable quantities use integers; human-readable
hours are derived at the API boundary.

| Quantity | Integer unit      | Accrual                                                            |
| -------- | ----------------- | ------------------------------------------------------------------ |
| Uptime   | seconds           | While a runtime is bound, including installation and pause cleanup |
| CPU      | millicore-seconds | Pinned CPU allocation multiplied by runtime uptime                 |
| RAM      | MiB-seconds       | Pinned RAM allocation multiplied by runtime uptime                 |
| Disk     | GiB-seconds       | Retained home capacity from its attachment receipt until release   |

These allocation quantities attribute capacity to owners; they are not the
platform cap's cloud source, which is an operator observation (see
[platform spend metering](CMA_Platform_Spend_Metering.md) §2.2).

CPU core-hours divide millicore-seconds by 3,600,000. RAM GiB-hours divide
MiB-seconds by 3,686,400. Disk GiB-hours divide GiB-seconds by 3,600.
These quantities measure allocated capacity, not sampled CPU load, resident
memory or filesystem bytes. Paused Sandboxes stop CPU/RAM accrual while their
retained disk continues accruing. Spare capacity is not attributed to a user
before Sandbox admission.

The middleware records created, started, paused, resumed, terminated, resource
changes, state changes and periodic checkpoints. The checkpoint worker closes
ongoing intervals every five minutes. Read requests project the open interval
without writing the ledger. Replica overlap and retried transactions serialize
on the same usage document, so interval quantities are not counted twice.

Existing Sandboxes receive a baseline when first observed. `complete_lifecycle`
is false for these rows: CMA does not fabricate usage before metering existed.
The UI shows the tracking start and identifies partial histories. Usage state
and events are retained after the Sandbox aggregate and its ordinary event log
are purged. Purge refuses any remaining runtime or home allocation.

During the first rolling upgrade, older replicas can still write lifecycle state
without metering. Every new write checks the ledger's source sequence and revision
against the aggregate before mutation. A mismatch preserves confirmed totals,
excludes the unknown open interval, emits a new baseline and marks lifecycle
coverage partial. Checkpoints apply the same check. No usage is invented or
exported for an unobserved transition, even when the final allocation is unchanged.

## Integration API

Every endpoint requires the usual CMA authentication. Owner reads require
`cma:sandbox:use` or visitor Launch authority; admin reads require
`cma:admin:sandboxes`. Sharing an Agent or Sandbox does not grant access to its
owner's usage ledger. A cross-owner detail request without admin authority
returns 404. Responses use `Cache-Control: private, no-store`.

| Method and path                                     | Result                                                                    |
| --------------------------------------------------- | ------------------------------------------------------------------------- |
| `GET /api/v1/usage`                                 | Owner totals, Sandbox count, running count, coverage and observation time |
| `GET /api/v1/usage/sandboxes`                       | Owner's retained per-Sandbox totals and current allocations               |
| `GET /api/v1/usage/sandboxes/{id}`                  | One owner's Sandbox; also readable by an authorized operator              |
| `GET /api/v1/usage/sandboxes/{id}/events`           | Immutable, ordered lifecycle and usage intervals                          |
| `GET /api/v1/admin/usage?owner={subject}`           | Global totals or one owner's totals                                       |
| `GET /api/v1/admin/usage/sandboxes?owner={subject}` | All Sandboxes or one owner's Sandboxes                                    |

Lists accept `limit` (1–200, default 50) and opaque `cursor`, returning `next`.
Event cursors are usage revisions. Timestamps are UTC Unix seconds. Integer
quantities are the integration authority; displayed decimal hours are derived.
`transaction_id` is stable across retries and identifies one closed interval.
The OpenAPI documents include the `usage` tag and complete response schemas.

TypeScript consumers use `client.usage.summary()`, `.sandboxes()`,
`.sandbox(id)`, `.events(id)`, `.adminSummary({ owner })` and
`.adminSandboxes({ owner, cursor, limit })`.
Rust consumers use `client.usage().summary()`, `.sandboxes(&page)`,
`.sandbox(id)`, `.events(id, &page)`, `.admin_summary(Some(owner))` and
`.admin_sandboxes(Some(owner), &page)`. Rust retains exact integer quantities
as `u64`; the TypeScript SDK rejects quantities beyond JavaScript’s safe integer
range. Both SDKs replay the shared backend contract corpus.

## Persistence and verification

Agent Manager owns `sandbox_usage` and `sandbox_usage_events`; isolated database
roles need the same collection privileges as its other owned collections.
Shared-schema deployments initialize them additively. Existing Sandbox document
schemas are unchanged. Usage persistence uses majority concerns and snapshot
transactions, and does not send external billing requests inside lifecycle writes.

The focused tests cover lifecycle arithmetic, retained-disk interval clipping,
duplicates, backward clocks, incomplete baselines, overflow, ownership, concurrent
Mongo checkpoints, transaction rollback, restart reads, direct attachment saves,
summary aggregation and history retention after purge. The Mongo tests require
`CHRONO_TEST_MONGODB_URI` naming an isolated replica set and are included in the CI
Mongo contract suite.

## Optional direct Lago exporter

Keep this exporter disabled for the NyxID prepaid lifecycle path below. It bypasses
NyxID admission and its internal usage ledger; prepaid windows use NyxID's single
reconciler instead. The following describes the existing optional direct exporter.

NyxID's current billing provider is Lago. Its customer external ID is the NyxID
owner subject and its subscription external ID is `{owner}:{plan_code}`. CMA
uses those existing identities and does not change NyxID code. Enable delivery
only after verifying the organization, effective API base and subscription plan.

Create three Lago billable metrics with `aggregation_type: sum_agg` and
`field_name: quantity`, then attach standard charges to the existing plan:

| Metric code | Price per integer unit |
| --- | --- |
| `cma_sandbox_cpu_millis_seconds` | CPU core-hour rate / 3,600,000 |
| `cma_sandbox_memory_mib_seconds` | RAM GiB-hour rate / 3,686,400 |
| `cma_sandbox_disk_gib_seconds` | Disk GiB-hour rate / 3,600 |

Use decimal arithmetic and the plan's currency. No nonzero prices are supplied
by CMA. A zero-price rollout can validate metering and invoice attachment before
paid rates are chosen. Preserve all existing plan charges and verify the effective
charges on existing subscriptions, which may override the public plan.

The exporter validates the subscription's exact owner, external ID, active status,
calendar billing, plan and all three effective charges before posting. It also
requires the customer's effective timezone to be UTC or Etc/UTC. It does not create
a second subscription, choose another owner, or post customer-only events. Closed
intervals split at UTC midnight so resource-time stays in its calendar period.
Other customer billing timezones require an interval-boundary extension before
delivery can be enabled for those accounts; their ledger remains retained.

Each ledger row is also an outbox item. A majority write freezes the exact target,
subscription, timestamps, decimal integer quantities and metric-specific
transaction IDs before the first HTTP send. All retries reuse that payload,
including after a crash or plan-default change. Expiring claims permit overlapping
workers; Lago transaction IDs provide provider deduplication after an ambiguous
acceptance. Requests have a 20-second deadline, refuse redirects and keep credentials
out of logs. Failures retry after five minutes without blocking lifecycle work.

`billing_delivered` means provider HTTP acceptance, not invoice finalization.
Operators can inspect `billing_attempts`, `billing_last_attempt_at`,
`billing_error`, `billing_retry_at` and the immutable `billing_payload` in the
owned usage-event collection; logs include the usage transaction ID and a bounded
error class. Provider asynchronous validation and finalized invoice totals must
also be checked during activation. Intervals exceeding 1,000 provider events are
retained with an explicit backfill error rather than silently truncated.

These dedicated metrics can appear in Lago current usage and invoices consumed
by NyxID. The optional direct-to-Lago exporter does not create rows in NyxID's
internal `/billing/usage` ledger. CMA does not enforce NyxID prepaid-credit
reservations or an overdraft policy. Those exporter and adapter limitations must
remain explicit when choosing paid pricing. Billing and the exporter are disabled
in the checked-in manifest until provider configuration and the companion
lifecycle contract are verified.

## Measured resource reports

See [Measured Sandbox resource reports](CMA_Sandbox_Metrics.md) for the new
owner/admin report APIs, CPU/working-set/PVC semantics, coverage, retention,
read-model migration, monitoring permissions and production verification.
The existing allocated usage ledger and billing quantities keep their meanings.

## NyxID prepaid lifecycle adapter

The adapter implements the producer contract in
[NyxID #2204](https://github.com/ChronoAIProject/NyxID/pull/2204), superseding draft
#2156. `CHRONO_NYXID_SANDBOX_BILLING_ENABLED` defaults to false. Disabled means no
funding HTTP calls and no lifecycle behavior change. The configuration rejects
simultaneously enabling the direct Lago exporter (`CHRONO_USAGE_BILLING_ENABLED`).
NyxID's reconciler is the only emitter for these prepaid windows.

With the CMA adapter enabled, an explicit NyxID quote refusal for a disabled
fleet/cohort admits a never-funded Sandbox generation without billing. CMA
persists `unbilled_generation` and returns no runtime funding deadline. Repeated
worker calls, restart, pause and the renewal sweep cannot reserve or charge that
generation. A later lifecycle generation rechecks eligibility prospectively.
Generic producer outages, authorization failures and insufficient funds do not
mean exclusion; pending/confirmed reservations always retain recovery and
settlement authority. The NyxID deployment must check rollout exclusion before
paid dependencies. See the [chronoai pilot runbook](CMA_Billing_Pilot.md) for the
contract acceptance gate, configuration, cohort scope and drain-before-disable
rollback procedure.

The provider-neutral lifecycle port admits an exact runtime generation before
creation/resumption and returns the confirmed exclusive deadline. MongoDB's
`sandbox_billing_state` collection persists the owner/Sandbox binding, source
lifecycle operation, UUID v4 funding operation, complete quote (including prices),
exact pending reservation, confirmed receipts, and ordered evidence outbox.
Majority writes with compare-and-swap revisions prevent replicas from replacing
one another's pending requests or receipts. Receipt recovery precedes replay of
the identical request; a 404 never causes a replacement idempotency key.

Windows last at most 300 seconds and may end earlier at a UTC month/rate boundary.
A five-second sweep checkpoints retained usage and renews only during the final
30 seconds, preserving the quote, operation, generation and immediate predecessor.
Confirmed receipts are persisted before provider desired state is extended.
Native AgentSandbox uses the **core Sandbox** `spec.shutdownTime` with
`spec.shutdownPolicy: Retain`, fenced by exact runtime identity, UID and resource
version. The controller removes the Pod/Service at expiry and retains the home;
CMA never sets the destructive Claim expiry policy. Other runtime backends reject
funded creation/resumption unless they implement this deadline capability.

A native deadline is provider control-plane desired state. It cannot guarantee
physical termination during a node/control-plane partition. Failed renewal or
unconfirmed deadline updates schedule the existing durable pause lifecycle and
publish a prepaid-funding failure/notice through Sandbox status. Existing cleanup
can proceed without new funds. Credentials, provider errors and arbitrary HTTP
response bodies are never retained as funding error text. Requests time out after
five seconds, refuse redirects, bound bodies to 512 KiB, and persist bounded
exponential retry scheduling with jitter (or the producer's `Retry-After`).

Pause/retirement marks compute stopped after provider teardown. Settlement waits
for the lifecycle usage write and actual elapsed observation of the complete
window, including explicit zero-allocation tails after retirement. Each payload
contains original contiguous retained timestamps/revisions and integer arithmetic;
CMA does not project a future tail, infer missing use, or split a window into
partial finalizations. The payload is frozen durably before its first POST. The
cursor advances only after acknowledgement, retaining an unchanged revision that
straddles adjacent windows. Partial delivery, response loss and restart replay the
same payload. Evidence is retained independently of the Sandbox's purge lifecycle;
a missing observation blocks settlement rather than inventing a refund.

Paused homes renew disk-only windows while coverage remains live. A funding
failure never authorizes deleting retained storage; unfunded residual disk is
platform cost pending the retention policy. A stopped or expired compute
generation cannot resume from its old receipt. Replacement waits for prior
coverage to expire and all earlier evidence to be accepted, then requires the
next consecutive generation, a fresh quote and a new lifecycle operation. CMA
fails closed if its runtime generation sequence skips the NyxID contract. Rate
cutoffs similarly pause and require a new owner-authorized operation; they cannot
silently renew an old quote at a new tariff.

Configure the HTTPS API base ending `/api/v1/billing/sandboxes`, the canonical
producer service-account UUID reference, and an environment variable name in
`CHRONO_NYXID_SANDBOX_BILLING_CREDENTIAL_REF`. Supply the named variable through
existing secret injection. Its value must be a General-purpose service-account
JWT with `billing:sandbox:write`. NyxID derives producer authority from the verified
JWT; CMA's configured identity is diagnostic, not an authorization header. Token
rotation is picked up when the process environment is refreshed; no credential
value belongs in configuration manifests, receipts or documentation.

Keep both CMA and NyxID admission flags off until the companion contract,
compatible native controller, synchronized approved prices, owner authorization
and price consent, and controlled subscription/invoice acceptance are verified.
This change supplies no public arbitrary-owner billing proxy or pricing editor.
To stop new paid admission while draining existing liabilities, use NyxID's
admission kill switch and leave CMA recovery running. Disabling CMA's adapter also
stops its HTTP recovery/evidence sweep; durable outbox records remain for a later
restart, and existing native deadlines still expire. Do not roll back to readers
that cannot understand records already written.

Operators can inspect `revision`, `runtime_generation`, `pending`, `current`,
`windows`, `evidence_cursor`, `stopped`, `last_error`, `retry_at`, and `attempts` in
`sandbox_billing_state`. Never manually advance a cursor or delete an ambiguous
pending request. Repair the underlying funding/authority/evidence problem and
allow replay, retaining the original ledger and accepted quote. Local fake-provider,
fake-HTTP, native Kubernetes HTTP, and isolated Mongo tests verify adapter recovery;
they do not certify live NyxID/Lago invoices or physical partition behavior.

The [adapter verification record](CMA_NyxID_Billing_Verification.md) documents
the exercised failure cases, measured coverage and local environment limits.

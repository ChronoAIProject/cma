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

## NyxID billing integration

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
by NyxID. They do not create rows in NyxID's internal `/billing/usage` ledger, and
CMA does not enforce NyxID prepaid-credit reservations or an overdraft policy.
Those limitations must remain explicit when choosing paid pricing. Billing is
disabled in the checked-in manifest until the provider configuration is verified.

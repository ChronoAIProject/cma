# Bounded lifecycle results

Refs #1164, M-FUNC-05, TC-047 and TC-048. Agent Manager owns this projection.

`GET /api/v1/sandboxes/{id}` adds optional `lifecycle.result`. Creation uses
`created_at`; pause, revive and image rotation use the existing operation
`requested_at`; retirement uses the atomically appended Retiring event, joined
to an exact applied retirement command when it originated in the command queue. No
observer-local clock start, lease renewal, retry time or `updated_at` sets the
budget. Every replica projects the same persisted start and ninety-second due
time. Observation uses UTC wall time; normal host clock synchronization remains
an operational prerequisite.

The separate durable command endpoint also adds optional `lifecycle.result`
while its receipt is Queued. It uses the command's original `admitted_at` and
advertises observation of that receipt only; resource authority is checked on
the resource itself. Applied is a committed transition, never runtime readiness.
Pause/revive application carries that original timestamp into lifecycle
authority, and retirement observation joins its exact applied receipt to the
Retiring event. Status events, work schedules and allocation intervals keep the
apply timestamp; only the result deadline carries admission time. Queue delay,
replay and worker restart never restart the budget.
Rejected commands retain their existing typed reason and omit the transient
result. Neither the 300-second queue-expiry policy nor execution deadlines change.

Before the deadline the result is `in_progress`. At the deadline it becomes
`recoverable_pending`, code `lifecycle_wait_exceeded`, with `continuing: true`.
It does not modify execution, cancel a load, create a conversation, or publish
Ready. A completed operation clears the transient result; actual Sandbox/Agent
status remains authoritative. Failed operations retain their typed `failure`;
an operation retained for recovery can also project `failed` with
`lifecycle_failed` and `continuing: false`.

The owner detail advertises `observe` and, when the caller holds owner delete
capability and the model and commit successor checks admit retirement, `retire`. `observe` means
GET the same Sandbox after `retry_after_seconds` (five) while continuing. A
failed result omits this optional interval. `retire` means DELETE
the same Sandbox with an Idempotency-Key. A fresh GET revalidates actions before
use; ordinary authorization and concurrency checks still apply. Summary views
advertise observation only. Shared grants do not acquire deletion authority.
Retirement already in progress advertises observation only. Repeated DELETE
observes the admitted retirement. Retained-home recovery has separate proof and
consent requirements and is not advertised speculatively by this result.

The retained initialize/load inactivity deadline remains 300 seconds, renewed
by durable output acknowledgements, with a 960-second absolute bound per call.
The existing adapter regression proves a 90-second initialize followed by a
90-second retained load succeeds; shortening execution to the public result
budget would regress this case. The intermediate result permits owner-directed
retirement while continuing slow recovery by default.

Owner retirement cancels the durable lifecycle operation and clears its lease
inside the Sandbox revision transaction. Stale reopen publication must fail its
captured parent/Agent fences. Cleanup uses the original runtime and home
identities, stops Agents through their existing fences, and requires provider
Claim/Sandbox/Pod finality before release. The regression injects a real
ConversationReopen rejection and also races a blocked reopen against owner
retirement, delayed deletion and a lost delete response. It checks preserved
conversation identity, no Ready publication, released home only after finality,
no remaining Claim/runtime/work lease and one effective provider deletion.

## Reader and rollback record

Migration identity: `2026-11-16-am-lifecycle-results` (expand projection only).
Source/target durable schema and indexes are unchanged. Writers, strict Sandbox
and lifecycle readers, worker leases, event variants, replay receipts, runtime
homes, command keys and runtime versions are unchanged. The read-only retirement event query uses existing `sandbox_events_sequence`;
its optional exact command lookup uses the existing ledger fields. Each query
has a two-second server bound. The historical command lookup can scan the
ledger (no new index/DDL); timeout returns typed unavailable rather than
inventing a younger admission time. Monitor this read on large ledgers. No backfill, DDL, writer gate or runtime promotion is required. All API
and SDK changes are additive; new clients accept absence and old clients ignore
unknown response fields. Data rollback remains at the existing reader floor;
rolling back this projection loses the public 120-second guarantee. Keep the
latest compatible API and command/recovery workers for that guarantee: older
queued-control workers can restart the observation deadline when applying a
receipt. Drain them before claiming the 120-second guarantee. No deployment
surface changes apply.

Local evidence is recorded with the implementation commits and final verification
report. Production/staging acceptance, 100/100 cycles and actual p95 readiness
measurements remain release-owner work under the acceptance runbook.

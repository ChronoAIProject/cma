# Operator admission control

PR-A of #1478 adds one platform-wide operator stop for new work. It is
independent of spend metering and requires `ConfigurePlatform`. It never
cancels a turn, pauses a Sandbox, changes an allowance, or closes observation
streams. Capacity ceilings and existing-consumer reserves belong to PR-B;
control rate budgets belong to PR-C.

## State and storage

The Agent Manager owns `platform_settings`, document `_id: "platform"`. The
new top-level `admission_control` field is outside the strict `settings`
record. Missing control means revision 0, open. A present malformed or
unsupported control is represented as unreadable: Fresh admission fails closed
with `admission_paused`, while admitted work continues. Admin reads and writes
reject the invalid record without repairing it. Its independent CAS revision
never increments `settings.revision`.
Settings replacement preserves this top-level field.

```text
AdmissionControl { revision, operator_stop?, last_change? }
OperatorStop { since, revision, actor, reason }
AdmissionChange { revision, at, actor, reason, old_stop?, new_stop? }
```

Times are Unix seconds. Reasons contain 1–512 UTF-8 bytes, are nonblank and
contain no control characters. Stop and change identity, revision and evidence
are validated together. The original stop's actor, time and reason survive
duplicate stop requests. These details are visible only through the authorized
admin surface; public refusals carry fixed text.

Every actual transition writes `last_change`, one existing-format `activity`
row and one immutable change record in the **same Mongo transaction**. The
change record lives in the existing `platform_settings` collection:

```text
{ _id: "admission_control_change:<revision padded to 20 decimal digits>",
  document_schema_version: 1, record: AdmissionChange }
```

The strict change preserves the operator, time, reason and complete old/new
stop configuration for each use, including after later stop/resume cycles.
Activity vocabulary remains `configure`, resource `admission_control`, revision
and status; the immutable record supplies the reason and structured old/new
state without changing that format. `last_change` remains the latest transition
on the singleton. GET validates that its recent window is complete, consecutive
and linked to `last_change`; missing, malformed or unsupported history makes
that admin read unavailable. There is no backfill or invented historical
evidence. No-op requests write neither activity nor change records.
The revision-derived unique key and same-transaction insertion prevent duplicate
history on concurrent stops or an uncertain commit. An unknown outcome retries
only the same commit; it never reruns the transition body.

`GET /api/v1/admin/admission` returns the current control plus `history`, at most
20 changes in descending revision order, read in one snapshot. The query ranges
over `_id >= "admission_control_change:"` and `_id < "admission_control_change;"`,
sorted descending. Existing singleton readers select `_id: "platform"`; they
do not decode history rows as settings. Retirement treats `platform_settings`
as Independent and selects none of its records, so owner retirement retains
these platform audit records. There is no new collection, index, catalog entry
or activity format. History records have no TTL; retention follows existing
platform settings backup and recovery custody.

## Refusals and continuation

The domain `PlatformAdmissionState` combines spend closures with independent
operator control (open, stopped or unreadable). `AdmissionRefusal` has `Budget`
and `Paused` variants; a future capacity variant can extend it. Spend refusals retain their
existing behavior and precedence, including 402 `platform_budget_exhausted`.

| Operation | Cause | Stop or unreadable control | Existing spend refusal |
| --- | --- | --- | --- |
| New public Sandbox, Workspace, Agent or inline lifecycle operation | Fresh | Refuse | Existing resource checks retained |
| New Launch | Fresh | Refuse | Unchanged |
| New message through REST, AG-UI, Responses or Event Gateway | Fresh | Refuse | Unchanged; spend takes precedence |
| New binding hierarchy or source-user BindingConversation | Fresh | Refuse before durable admission | Existing resource checks retained |
| Owner revival or new Sandbox Revive control | Fresh | Refuse | Unchanged |
| Admitted Launch resource creation, profile Agent reuse, first message or revival | Continuation | Continue once under original identity | Unchanged |
| Admitted lifecycle, pinned-profile and binding provisioning | Continuation | Continue under original identity | Unchanged |
| Admitted command revival or verified external media continuation | Continuation | Continue | Unchanged |
| Turn start, auxiliary request and settlement of accepted work | Continuation | Continue | Unchanged |
| Exact accepted replay | Original receipt | Return original result | Existing spend behavior retained |
| Stop, cancel, interrupt, approval, pause, retire, deletion, reads and existing streams | Control/observation | Available | Unchanged |
| Never-seen subject callback, registration entry and signup options | Fresh | `registration_closed`; options advertise false | Unchanged |
| Existing owner's sign-in | Existing identity | Available | Directory lookup still precedes admission state |

Public refusal is HTTP 503 `admission_paused` with `Retry-After: 60` and fixed
guidance: "New work is temporarily paused. Existing work continues." Responses
and AG-UI project the same typed admission refusal. Registration keeps its
existing HTTP 503 `registration_closed` contract. Neither the reason nor
operator identity, revision, fleet counts or spending details appear in a
public refusal. Clients should honor the retry delay and retain the same key
for an uncertain accepted request; they must not spin on the refusal.

Continuation is server-attributed from original durable operations, origins,
binding fences or validated parent receipts. Public creation validates Manual
origins. A caller cannot choose continuation in the public request. Replay,
leases, operation IDs and existing fingerprints continue to prevent duplicate
effects; resuming the switch never replays work automatically.

## Propagation and the 60-second requirement

Enforcement and reads are always enabled; there is no process cache for the
operator control. Every new message parses the settings document already read
by its admission transaction, adding **zero Mongo commands**. Its existing
settings fence serializes conflicting stop writes and the admission retries
against current state. Stop checks both budgeted and unbudgeted owners.
`start_turn` uses that snapshot without enforcing operator control. Unreadable
control also leaves settlement, auxiliary work, the spend stop poll, evaluation
and spend reads available; their existing spend validation is unchanged.

Zero-liability Sandbox, Workspace, Agent and Launch admissions read control in
their transaction snapshot without writing either hot settings or period
document, following platform spend contract §4.1. New lifecycle and binding
operation boundaries retain their original single-document writes and use a
plain majority read after replay/existing-state validation, immediately before
the write. They add no session, transaction or application retry loop. Reading
control in a new read-only transaction would provide no stronger stop guarantee.
BindingConversation and reprovision paths already used transactions and retain
their in-snapshot checks. A snapshot or majority read that started before the
stop may still be followed by a committed admission. The window is the
remaining duration of that admission, not a reconciliation tick or TTL.

Current loops bound attempts, **not total transaction duration**: Sandbox and
Workspace retry at most 16 times; Launch at most 8; Agent limits retry starts
to 5 seconds. Mongo operations and commit have no explicit overall deadline;
the 30-second reconciliation wait can leave work running in the background.
Consequently the code establishes next-snapshot enforcement but does **not**
prove a hard ≤60-second bound on late commits of pre-stop snapshots. A
one-line retry-loop deadline cannot establish that bound. The smallest safe
follow-up needs a shared snapshot deadline, bounded database/commit IO and
commit-ambiguity handling; it is outside PR-A's permitted deadline change.
Registration checks control at the callback before the directory write; those
steps are not one transaction and have no proven overall IO deadline either.
PR-A does not add PR-B's registration ledger.
The release owner must settle request-arrival versus commit-time measurement
and obtain the TC-100/M-OPS-03 drill evidence before claiming qualification.

The new durable lifecycle/binding operation gates read operator control only.
They preserve #1741's spend timing: SpendStop is checked at their existing
resource steps, including for continuations. If both controls are active at
one of these new operation boundaries, the immediate refusal is
`admission_paused`; full spend precedence applies at the inherited spend
choke points. This avoids changing spend admission semantics in PR-A.

## Operator API and concurrency

| Method and path | Request | Result |
| --- | --- | --- |
| `GET /api/v1/admin/admission` | None | Current state, `writes_available` and newest 20 changes in `history` |
| `POST /api/v1/admin/admission/stop` | `{reason, expected_revision}` | Stop or original current stop |
| `POST /api/v1/admin/admission/resume` | `{reason, expected_revision}` | Resume the exact current stop |

All three require `ConfigurePlatform`. The GET response is flat:
`{revision, operator_stop, last_change, writes_available, history}`. Stop and
resume retain `{revision, operator_stop, last_change, writes_available}`.
A new transition requires the current revision. Concurrent stops are first-writer-wins;
already stopped returns current state even if the supplied revision is old.
A stale resume cannot lift a later stop and returns HTTP 409
`admission_control_conflict`. Resume while already open is a no-op only at
the current revision. Each successful transition increments revision once;
transactions retry conflicts, and uncertain commits never rerun blindly.

`CHRONO_ADMISSION_CONTROL_WRITES_ENABLED` accepts exactly `true` or `false`,
defaults to `false` in code and ConfigMap and is independent of budget gates.
Closed writes return HTTP 503 `admission_control_writes_unavailable` for both
stop and resume. A closed gate does not lift a stored stop. Follow the
[operations runbook](CMA_Operations.md#operator-admission-stop).

## Data migration record

| Required field | Record |
| --- | --- |
| Identity, phase, owner | `2026-11-12-am-operator-admission-control`; #1478 PR-A; Agent Manager/store owner; additive expand with default-closed writers. Exact source identity is the ordered patch series against base `821790b422b336684256abbf2b00d30386f40e99`, with checksums in the PR evidence. Deployment operator and image digest remain release-owner evidence. |
| Stores and writers | `platform_settings.admission_control`, immutable `platform_settings` change records and existing `activity`; only the capability-gated stop/resume service writes control. Enforcement readers include requests, authentication and all creation/Launch/binding workers. No runtime, browser journal or external-service writer is added. |
| Format and preservation | New strict control, stop, last-change and versioned immutable change structures; existing envelope schema, collection catalog, indexes, validators and activity TTL unchanged. Independent revision preserves settings CAS, resource IDs, homes, histories, keys and operation/command receipts. |
| Reader floor | Every serving/recovery API process must implement this control and continuation contract before writes open. Older APIs ignore the top-level field, making enforcement ineffective. Exact immutable candidate/recovery digests and old-artifact execution remain unverified locally. Retained runtime images and SDK assets are unaffected by the storage shape. |
| Writer gate | `CHRONO_ADMISSION_CONTROL_WRITES_ENABLED=false`; `ConfigurePlatform`; all replicas must run the compatible image before activation. Reads, admitted work and settlement continue when writers close. |
| Backfill/startup | No backfill, transform, scanner, new collection, index, validator or TTL change. Existing collection initialization remains in use. No migration executor/lease is introduced; control updates use independent revision CAS and Mongo transactions. Missing field remains open. History is appended only for actual transitions; no historical details are fabricated. |
| Prerequisites/checksum | Compatible #1741 reader baseline and this PR's readers; release owner records reviewed tree/executable checksum, exact image identities and replica drain receipts before gate activation. No production execution occurred locally. |
| Backup/dry-run | No destructive or irreversible transformation requiring a new backup. Existing backup/restore custody still applies to settings, immutable change history and activity. Local disposable replica-set tests exercise old missing-field data, new state and transaction failure; these do not certify production restore. |
| Recovery | Before first control write, retain current compatible recovery policy. After writes, keep a reader implementing this shape. **Never deploy or roll back while stopped**; resume using the current stop revision before any release change. Preserve history, state and homes; use a compatible forward fix for failure. Closing the gate alone never reopens admission. |
| Completion | Local test logs and exact counts are recorded in PR-A notes/description. Production drill, candidate/recovery image identities, all-replica reader proof, ≤60-second measurement and production audit verification remain release-owner items. No production qualification or issue closure is claimed. |

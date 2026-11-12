# Operator admission control

PR-A of #1478 adds one platform-wide operator stop for new work. It is
independent of spend metering and requires `ConfigurePlatform`. It never
cancels a turn, pauses a Sandbox, changes an allowance, or closes observation
streams. PR-B adds capacity ceilings and existing-consumer reserves below;
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
operator control (open, stopped or unreadable). `AdmissionRefusal` has `Budget`,
`Paused` and `Capacity(CapacityKind)` variants. Spend refusals retain their
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

## Capacity ceilings and the existing-consumer reserve

PR-B adds a separate top-level `admission_ceilings` field on `_id: "platform"`.
It is deliberately outside `admission_control`: PR-A's strict control decoder
would reject an unknown nested field during recovery. Settings replacement,
stop/resume and ceilings replacement preserve each other's fields and revisions.
Missing ceilings mean unlimited; a present unreadable policy refuses fresh
admissions governed by these checks. It never blocks reads, settlement or an
already admitted continuation. A configured zero is a real ceiling of zero.

```text
AdmissionCeilingsState { revision, ceilings, last_change? }
AdmissionCeilings {
  registrations?, retained_homes?, active_sessions?, owner_active_sessions?,
  existing_reserve: { retained_homes?, active_sessions? }
}
AdmissionCeilingsChange {
  revision, at, actor, reason, old_ceilings, new_ceilings
}
```

Every limit and reserve is an optional unsigned 32-bit integer. No capacity is
guessed or enabled by default. An absent reserve means zero, and a reserve must
not exceed its configured ceiling. The effective retained-home ceiling is the
minimum of the operator ceiling and a nonzero `CHRONO_SANDBOX_GLOBAL_LIMIT`;
zero in that deployment setting remains unlimited. If the deployment cap is
smaller than the reserve, the public share saturates at zero. Existing consumers
may consume up to the effective ceiling; public subjects stop at ceiling minus
reserve. This is a reserved slice of shared fleet capacity, not an additional
per-subject allowance. Lowering a ceiling retains all existing resources and
turns, and refuses only additional admissions.

### Registration cohort and activation

A subject is public only when its callback has written a registration receipt
under the newly activated path. All other owners, including API and binding
subjects, use the existing-consumer share. The callback checks retained directory
records, raw persisted sign-in subjects and owned resources before classifying
an unseen subject as public. A failed best-effort directory write therefore
cannot turn an existing owner into a public signup. Retained retired resources
also prove prior ownership. Exact public registration replays precede these
checks and remain counted once after their directory/sign-in records exist.

The first process startup with `CHRONO_ADMISSION_CEILINGS_WRITES_ENABLED=true`
creates the activation fence with `$setOnInsert`. Its immutable `activated_at`
is the cohort boundary. Before that event no registration receipts or counter
are written. After activation registration tracking continues even if the
configuration write gate later closes, including while registrations are
unlimited. Closing the gate prevents configuration changes; it does not reset
that retained cohort boundary. Deploy compatible serving and recovery APIs and
drain incompatible predecessor writers before opening the gate. A running
compatible callback receiver discovers activation without restarting: while its
state is unknown or inactive, a never-seen callback reads the marker once by
exact `_id`. Only a positive result is cached, because activation is immutable.
A known directory owner returns after its existing directory read and adds no
activation query. Existing retained-owner evidence still precedes registration
reservation. There is no registration-callback exclusion or restart requirement
for compatible receivers at activation.

The existing `sandbox_admissions` collection stores these additional shapes:

```text
{ _id: { scope: "registration_capacity" }, document_schema_version: 1,
  activated_at: <Unix seconds>, count: <nonnegative integer> }
{ _id: { scope: "registration", subject: <subject> },
  document_schema_version: 1, registered_at: <Unix seconds> }
{ _id: { scope: "global_sandbox_capacity" }, document_schema_version: 1,
  fence: <integer>, pending_launch_homes: <nonnegative integer, absent means zero> }
{ _id: { scope: "launch_home_capacity", launch: <Launch id> },
  document_schema_version: 1, owner: <subject>,
  state: "reserved" | "reused" | "consumed" | "released", sandbox_id?: <id> }
```

A registration transaction inserts one unique subject receipt and increments
`count` on the fence only while below the configured registration limit. The
counter is O(1), never reconstructed by scanning registration scopes. Conflicts
and duplicate keys retry within a finite budget; exact duplicates consume no
additional capacity. Unknown commit outcomes retry only the original commit.
Existing identities sign in even when settings are unreadable or capacity is
full. Signup options use a point read of the counter when a registration ceiling
is set and advertise `register_available: false` at the boundary.

### Retained homes and active sessions

Retained homes reuse #1672's global transaction fence, live-home index hint,
bounded count and three-second database operation bound. Provisioning, paused
and retiring homes retain their slots until retirement completes. Public fresh
creation uses the reserved slice. Continuation steps of an admitted Launch keep
#1672's deployment hard-cap behavior and bypass the new operator home ceiling.
After the same durable activation exists, fresh Launch admission reserves a
pending home in the same transaction as the Launch when its resolved plan needs
a new home. The shared global fence's
`pending_launch_homes` counter participates in ordinary and Trigger capacity
checks. A keyed `launch_home_capacity` receipt preserves its owner and state.
Sandbox insertion converts a reserved slot to a live home in one transaction;
terminal/cancelled Launch commits release an unused reservation. Retryable
Launch failure retains its slot. Replays never reserve or release twice.
Reused homes need no additional slot; a retained marker makes any later new-home
replacement meet the numeric capacity authority. Predecessor Launches without
receipts retain their inherited continuation rule. Transfer/release reads are
settlement of already admitted reservations, including after a ceiling is unset;
they are not fresh capacity checks. New Launch admission with both the operator ceiling unset and the deployment
hard cap zero adds no capacity command. Before activation, a nonzero deployment
hard cap preserves the base behavior: Launch admission writes no new capacity
marker, pending counter or receipt, and workers enforce #1672's retained-home
cap with its original fence, index hint, limit and timeout. A nonzero env cap
alone never enables the new persisted shapes. After activation, configured
operator ceilings or a nonzero deployment cap enable Launch reservations, which
remain recoverable after the write gate closes.

Recovery writers must preserve `trigger_launches.home_capacity_admission` through
whole-document replacements and settle its matching receipt. Predecessor writers
can discard that marker and leak pending reservations. Removing a configured
ceiling does not make those writers compatible while retained reservations exist.

Binding hierarchy work retains PR-A's continuation classification for stop and
spend. Inserting an unreserved new binding home still grows fleet usage, so it
uses the readable operator ceiling and includes pending Launch reservations.
Binding consumers use the full existing-consumer ceiling. Existing-home return
and exact replay precede growth checks. An unreadable policy does not strand a
binding continuation; its deployment hard cap remains effective.
The fresh binding boundary rejects unreadable ceilings before retaining a new
home-producing binding, reusing its existing settings read. This prevents a new
request from acquiring continuation authority under an unreadable policy.

An active session is one Agent whose top-level status is `working` or
`waiting_approval`. Fresh messages that could make an idle Agent active check
the fleet and per-owner ceilings in `reserve_command`. Messages queued to an
already active Agent consume no additional slot and bypass capacity checks.
Validated binding command and execution fences attribute existing-consumer work
without a new lookup. Those commands use the full fleet share and still obey
the per-owner active ceiling; clients cannot choose this classification.
The authority is `start_turn`: the existing settings fence serializes concurrent
starts, and the Agent's top-level status and `TurnStarted` log commit in the
same transaction. Fleet counts use `{status, _id}`; owner counts use
`{owner, status, updated_at}`. Both use a ceiling-bounded count and `max_time`.
Unset checks return before any cohort read or capacity count; they add no Mongo
command to the inherited message/home transaction.

Queued commands have durable `agent_commands` records, but these are indexed
per Agent. There is no existing bounded indexed count of distinct pending Agents;
`work_pending` also includes unrelated work. Queued messages therefore do not
reserve active slots. Several idle Agents can pass the admission precheck;
only available slots can cross the authoritative turn-start transaction.
An accepted command refused there follows the owner-budget terminal path:
fixed typed failure, no runtime dispatch, no automatic replay. Retrying its
original identity returns that same terminal receipt. An unreadable policy at
this continuation boundary does not prevent already admitted work starting;
a readable numeric ceiling is still authoritative.

The new public refusal is HTTP 503 `session_capacity_unavailable` with
`Retry-After: 60`. REST, AG-UI and Responses preserve this type. Registrations
keep `registration_closed`; homes keep `sandbox_capacity_unavailable`.
No public refusal contains limits, counts, operator details or reasons.
Spend and operator-stop precedence remain ahead of capacity checks.

### Capacity operator API

`PUT /api/v1/admin/admission/ceilings` accepts
`{ceilings, reason, expected_revision}` and requires `ConfigurePlatform`.
It replaces the complete policy; omitted/null limits are unlimited. The reason
uses the same 1–512-byte bounded contract as stop/resume. CAS is independent
of both settings and stop revision. An exact no-op at the current revision
writes nothing; a stale revision returns 409 `admission_ceilings_conflict`.
A closed gate returns 503 `admission_ceilings_writes_unavailable`.

`GET /api/v1/admin/admission` retains the stop fields and `history`, and adds:

- `ceilings`: `{revision, ceilings, last_change, writes_available}`.
- `effective_limits`: the policy with the deployment home cap applied.
- `capacity`: optional observations for configured registration/home/session
  limits. Each has `{observed, bound, saturated}`; saturated counts are lower
  bounds. An unset limit has a null observation and skips its resource count.
  `capacity.pending_launch_homes` is the exact nonnegative pending counter,
  with an absent counter reported as zero. It is read once by exact `_id` with a
  three-second bound even when the retained-home ceiling is unset, so retained
  reservations remain visible after policy removal.
- `ceilings_history`: the newest twenty immutable full changes, descending.

Advisory state is computed only on demand. No timer writes the hot singleton.
Ceilings history uses `_id: "admission_ceilings_change:<revision padded to 20>"`
and `{document_schema_version: 1, record: AdmissionCeilingsChange}` in
`platform_settings`. Each change, singleton CAS and one existing-format
`activity` row commit together. The activity target is `admission_ceilings`;
immutable history retains actor/time/reason and complete old/new policies.
History uses an `_id` range and a 20-row limit; incomplete or inconsistent
history makes the operator read unavailable. No historical records are invented.

### Capacity migration and recovery record

| Required field | Record |
| --- | --- |
| Identity/phase/owner | `2026-11-13-am-admission-ceilings`; #1478 PR-B; additive expand, default-closed configuration writers; Agent Manager/store owner. Source base `192e7884814d062a8df92be70ba0bb2988bef33c` plus ordered review patches and checksums. |
| Stores/writers | `platform_settings.admission_ceilings`, immutable ceiling-change rows, `activity`; capability-gated operator PUT. `sandbox_admissions` activation fence and registration receipts: first enabled startup and callback transaction, including post-activation tracking after gate closure. After durable activation, configured-capacity Launches preserve the additive `trigger_launches.home_capacity_admission: true` marker across claims, handoffs and full replacements. Fresh Launch admission and terminal/cancelled Launch commits also write pending-home reservation receipts/counters in `sandbox_admissions`; Sandbox admission transfers them atomically to retained homes. Command failure metadata also writes `agent_commands.admission_capacity_failure` at authoritative refusal. Legacy first-message workers persist the new nonretryable startup failure in `agents` and their Agent-log publication in the same conversation transaction. No runtime files, client journals or external effects. |
| Formats/preservation | Strict independent state and change records; new structured registration keys and counter; structured `launch_home_capacity` receipts and `global_sandbox_capacity.pending_launch_homes` preserve pending home authority; no replacement of PR-A control, settings or operation identity. Existing collections/catalog/validators/TTL and home identities/history remain. The auth store adds the nondestructive `auth_api_bearers_subject` index `{subject: 1, _id: 1}` for bounded prior API-custody evidence. Command records retain predecessor `execution_failed` bytes; the top-level sidecar restores `session_capacity_unavailable` to current readers. Historical FirstMessage execution without a common command stores the typed nonretryable `session_capacity_unavailable` startup failure; its exact unattempted-failure whitelist now recognizes that retained state. PR-A cannot validate that newly failed FirstMessage and is excluded from recovery. |
| Reader floor | Every serving/recovery API must implement cohort, ceilings, Launch reservations and typed terminal recovery before activation. Before activation, even a nonzero deployment cap writes no new Launch capacity shapes and retains base worker admission. Drain incompatible predecessor writers before opening the activation gate; compatible receivers discover the marker without restarting. PR-A ignores the new top-level settings field without corrupting control, but does not enforce ceilings or track registrations; it is not an authorized recovery artifact once activated. Exact immutable API/recovery digests and old executable matrix remain release-owner evidence, not locally certified. Runtime images are unaffected. |
| Gate/cohort | Exact boolean `CHRONO_ADMISSION_CEILINGS_WRITES_ENABLED=false` in code/ConfigMap; `ConfigurePlatform`; one immutable first-open boundary for registration and new Launch reservation shapes. No registration receipt, home_capacity_admission marker, pending_launch_homes field or launch_home_capacity receipt is written before it, including with a nonzero env cap. Gate closure preserves tracking and all retained resources/reads/continuations. All replicas must agree before serving. |
| Migration/backfill/startup | No backfill, count scan, transform, deletion, new collection, validator or TTL. Auth startup idempotently creates the additive subject index; partial startup safely retries the same index definition. Existing data counts directly for homes/sessions; public registrations start at activation. One-time activation upsert is idempotent and never resets count/time; replacement uses revision CAS and bounded retrying transactions. Startup retains existing schema/index initialization; any added index is documented with its owning store. |
| Backup/dry-run | No destructive transformation or irreversible deletion, so no new backup operation is required by this slice. Existing settings/admissions/command/activity backup custody applies. Disposable-replica-set concurrency and rollback tests provide local rehearsal only, not production restore certification. |
| Recovery/abort | Close configuration writes, retain compatible enforcement and cohort tracking, preserve every ledger/history/sidecar/home. Prefer a compatible forward fix. Do not roll back to an API that ignores active ceilings or resets the cohort; gate closure cannot erase retained state. Candidate/recovery digests, replica drain and a coherent post-backup-write procedure remain release-owner requirements. |
| Completion | Local logs, configuration, sample counts, time window and patch identity are in PR-B evidence/description. Actual K, reserve peak×1.5, CPU/memory reservation, N+1/stream mapping, production drills and TC-070/TC-078 qualification remain outside local claims. |

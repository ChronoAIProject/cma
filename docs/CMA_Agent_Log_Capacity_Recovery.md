# #1351 — Agent log capacity recovery

Design ready 2026-11-11 SGT. Implementation branch:
`tl/cma-log-segments/capacity-recovery`. Part of #1351; bounded recovery consumers
are also part of #1349. Source baseline: `8db061e4af4ccbbb77d172f8dad98459b7ab3a26`.
This design does not authorize production recovery or claim implementation gates.

## Outcome and current cases

Recover storage capacity on the same Agent, retained home, native conversation,
creation identity and global event sequence. An exact recovery operation adopts
and seals the valid old prefix, creates one active successor, and durably records
its recovery work in the same transaction. Runtime confirmation is a later fenced
phase. Storage adoption alone never makes an Agent Ready and never sends input.

The 16:59 SGT fleet snapshot has nine capacity-failed Agents at head 10,001:
six under Failed parents, two under Retiring parents and one under
CredentialRotating. These are historical observations, not admission authority.
Every dry-run rereads current authorized lineage. There is no immediately proved
resumable member in that snapshot.

| Current condition | Disposition | Allowed next action |
| --- | --- | --- |
| Parent Failed | `parent_recovery_required` | Keep Agent failure/history; parent availability owner restores the original authority first; then fresh dry-run. |
| Parent CredentialRotating, Rotating, Pausing, Paused or Reviving | `parent_not_ready` | Wait for the recorded transition. Never cancel or supersede it. |
| Parent Retiring/Retired or workspace/worktree removing/removed | `cleanup_only` / `terminal_preserved` | Preserve retirement; continue only already-admitted cleanup with original receipts. Never reopen native conversation. |
| Agent owner stop/delete intent or terminal Stopped/Deleted | `cleanup_only` / `terminal_preserved` | Preserve intent and retained history. |
| Capacity-only Failed, ready exact lineage, valid retained native/home identity | `eligible` | Explicitly admit the exact reviewed recovery through both revisioned gates. |
| Original pending capacity-blocked reconnect under ready lineage | `continue_original_work` | S4 adopts in the original publication transaction, retaining original operation/lease/effect receipts. No second reconnect command. |
| Unsupported identity, corrupt/over-physical prefix, uncertain original native evidence | `blocked` with fixed reason | Keep evidence; repair through its existing separately authorized owner. |

The two Retiring-parent cases are successful *dispositions* when retirement and
cleanup are preserved; they are not recovered running Agents. No blanket
Failed→Ready rewrite and no change to the generic Agent transition graph.

## Read-only dry-run HTTP contract

`GET /api/v1/agents/{id}/capacity-recovery/dry-run` requires current
`OperateAgent` authority, including fresh resource/share policy and membership
where applicable. It remains available when recovery admission is disabled.
The successful JSON object contains exactly `agent_id`, `disposition`,
`observed_revision`, `through_seq` and `evaluated_at`. It exposes no original
input, runtime/home identity, open receipt, settings, operation fingerprint or
native-state proof. `evidence_required` is an observation that further certified
source work is needed; it is not admission or a recovery-success assertion.

The service rechecks current coherent ancestry before release and does not read
or append the log, create recovery work or contact the runtime. A retiring parent
continues to return cleanup-only intent. Existing first/queued messages and all
receipts remain unchanged.

Responses carry `Cache-Control: private, no-store`, including authentication
extractor refusals and dependency/authorization errors. Missing authentication
returns 401, a concealed foreign/unknown resource 404, insufficient current
operation authority 403, changed authority 409 and unavailable authority/source
metadata 503. The route has no POST/admission operation. Production admission
and completed-status routes remain separate acceptance work.

## Reuse and code boundaries

Accepted-work discovery reports bounded diagnostics independently of the new
admission switch. `cma_capacity_recovery_discovery_total{phase,outcome}` counts
one outcome per polled presence probe or discovery page, including dropped
operations. Phases are `presence`, `admitted_query`, `expired_query` and
`candidate_validation`; outcomes are `success`, `absent`, `deadline`, `driver`,
`invalid_evidence` and `cancelled` (at most 24 counter series per process).
`cma_capacity_recovery_discovery_seconds{phase}` records elapsed attempt time in
four phase series; a validation failure includes the preceding page query time.
The failure log retains fixed phase/reason, numeric Mongo code, transient and
unknown-commit label booleans, actual rows seen and monotonic elapsed milliseconds.
It omits driver messages, candidate content and resource identifiers. Queue delay
is not measured by this instrumentation. The existing scheduler error still
propagates: a missing index or invalid candidate is never treated as absence.
Presence deadlines, polling, cursors and recovery authority are unchanged.

The current storage checkpoint uses recovery carrier BSON format 2. Its strict
`result` witness records the original Agent result hash, log-batch hash, Agent
revision, global through sequence and publication timestamp. S4's immutable
publication digest binds that witness and the full recovery work with domain
`cma.capacity-recovery.result/v2`. The witness contains no input or Agent snapshot.
Earlier draft format-1 carrier rows are refused; this default-off carrier has
not been activated in production. The domain work record remains format 1.

Receipt reads verify the original admission, exact work revision and publication
result, current certified segmented source, historical seal coordinates and the
single retained completion status event. A later Agent revision or owner stop
does not rewrite the original result. Missing publication evidence remains
unresolved. Returning a completed receipt does not reconnect or activate the
native process; that remains a separately fenced runtime responsibility.

The integration plan assigns S5 capacity recovery, checkpoint/maintenance/
unpublished recovery and scoped retry pacing. S1 owns raw-BSON validation,
CapturedLog/range proof, format markers and reader compatibility. S4 owns the
single transactional publication/adoption helper, publication receipts,
projection writes and durable gate records. S2 owns common trait/config/module
registrations. S5 must not duplicate these schemas or edit their owned files.

Current `plan_agent_log_capacity_failure` preserves conversation state and
records the exact failed work step. Existing connection recovery only accepts
live Agents. Its connection completion can schedule DispatchFirstMessage, and
reattachment may replay pending managed context/settings. Therefore the recovery
worker uses a restricted attachment path: original open receipt + native prompt
identity, no first-message dispatch, no Message/Steer/managed-context submission,
no generic startup path. Readiness is published only after exact runtime/native
identity is confirmed and buffered output has an authorized durable recorder.

New S5 modules: core `agent_capacity_recovery` for the pure state machine and
classification; core `store/agent_capacity_recovery` for the narrow persistence
port; Mongo `store/agent_capacity_recovery/{admission,status}`; worker
`agent_worker/capacity_recovery`; REST `agent_capacity_recovery`; focused tests.
Existing S5 work-validation and recovery-consumer files integrate those modules.
New modules are registered by S2 before review; no dead unregistered draft PR.

## Stable request and persisted authority

A request identifies the exact original capacity failure/publication, not a new
message. The logical key derives from immutable Agent creation identity plus
original failure global sequence and recovery purpose. A request fingerprint
binds the reviewed source head/revision, failure evidence, parent lineage,
original open/effect receipt, native conversation/runtime generation and retained
home/claim identity. Repeating the same key/fingerprint returns the same recorded
admission/outcome; reusing the key with different facts conflicts.

Use existing SHA-256/domain separation and canonical contract encoding agreed
with S4. Hashes are internal authority evidence, not public diagnostics. A bounded
receipt stores scalar source/successor/operation identities, original failure
reference, gate revisions and immutable admission result. Growing event/command
sets remain indexed source/evidence rows; never copy a lifetime transcript or
command array into a receipt.

The request's admission timestamp and fingerprint stay fixed across retries.
Transaction validation uses a fresh current time for lease expiry; an old request
timestamp cannot revive an expired worker. Every mutation checks current
ownership/resource policy, retirement/history seals, parent revisions, Agent
revision/head, native/home identity and recovery/work lease fence.

## State machine and atomic boundary

1. **Observe (read-only).** Authorize owner/operator and read one coherent
   snapshot. Classify parent/Agent/cleanup state before expensive history work.
   Validate exact failure and open/effect evidence using S1/S4 bounded reads.
   Disabled/missing recovery gate still permits authorized dry-run/status.
   No implicit preparation, quarantine write or migration on this read.
2. **Adopt and admit (one transaction).** Check immutable existing recovery
   receipt first, then current authorization. New admission requires the writer
   gate, independent explicit recovery cohort and certified writer/reader floor.
   Write-fence both gate revisions and the parent cleanup/admission records.
   Refuse an unexpired prior lease; reserve a strictly higher work/recovery fence.
   Recheck the captured Agent revision, global head, creation identity and native/
   home identity. S4 commits valid legacy adoption/seal, empty active successor,
   v2 marker, bounded projections, recovery work and immutable receipt together.
   Keep the Agent's failure/non-ready presentation until runtime confirmation.
3. **Reattach (durable leased work).** The recovery worker claims only admitted
   work, with fresh authority and the pinned original home/native target. It
   invokes the existing supported reconnect operation with the original open
   fingerprint and recovered prompt identity. It never invokes input admission,
   first-message retry, new-conversation creation, runtime replacement or a
   generic resume that sends pending managed context. Unsupported outstanding
   mutation must be reconciled by its existing owner or recorded blocked.
4. **Confirm (one transaction).** Match exact native conversation, runtime/home
   authority and original open receipt. Fence current parents, history state and
   the still-owned unexpired recovery lease. Publish recovery outcome/current
   Agent state plus any real lifecycle event/projection/receipt in S4's existing
   publication transaction. Ready requires confirmed idle runtime; an already
   active turn keeps its verified Working/WaitingApproval state. Do not rewrite
   command status, original dispatch identity or checkpoint acknowledgment.
5. **Complete or blocked.** Immutable completed receipt remains discoverable
   after later appends. Retryable dependency failures keep durable phase and
   original key. Unsupported/corrupt/native mismatch gets an explicit blocked
   reason. New stop/delete/retirement supersedes reattachment, fences it and
   follows cleanup. Completion of the same historical operation never restores
   a subsequently stopped/deleted Agent.

Storage-only adoption adds zero events. For old head H, source raw rows remain
[1,H], the immutable seal ends at H, and active successor starts H+1 with zero
entries. The Agent global head stays H. Only actual later publication advances
it. No empty seal, sequence reset, moved rows or artificial recovery message.
At 10,000 / 10,001 / 10,064 / 10,128 and valid byte-reserve boundaries, adoption
requires no spare event slot. Greater than physical legacy capacity or failed
source validation is refused, not repaired by splitting corrupt history.

The exact durable work carrier is mapped onto S4's accepted admission/publication
record in the shared contract freeze; S5 does not add an independently mutable
flag that could claim successful adoption. A temporary non-ready recovery phase
must remain visible to compatible readers and unavailable to ordinary command
admission. No writer may publish a live status merely because a sidecar exists.

## Crash, retry and unknown commit

| Interruption | Required behavior |
| --- | --- |
| Before transaction commit | No successor, admission or Agent transition is visible. Known abort may retry exact request after current fences. |
| Commit succeeds, acknowledgment lost | Majority visibility barrier + one coherent read of exact publication/recovery receipt and successor/source identity. Matching receipt is success after later appends. |
| No receipt visible without proof of abort | Return `outcome_unknown`; retain key and phase. Never allocate another successor or infer runtime input was not sent. |
| Crash after adoption before reconnect | New lease holder resumes the recorded recovery; adoption is not repeated. |
| Crash after runtime reconnect before publication | Reconcile/reconnect the same original native/open operation under a new fence; do not send user input. |
| Ready publication acknowledgment lost | Reconcile exact completion receipt, do not resend input or replace later Agent state. |
| Lease replaced, parent changes, stop/delete wins | Old holder cannot publish, acknowledge recorder output, or release a usable attachment. Preserve superseding intent. |
| Recovery gate closes after admission | Exact admitted work and uncertainty settlement continue; new admissions stop. An old turn/lease is not unlimited rollover permission. |

There is no equality-of-current-head shortcut for commit reconciliation. Terminal
or ambiguous responses (`agent_startup_failed`, `runtime_lost`, etc.) remain
terminal/ambiguous even when capacity is restored. No test or implementation may
interpret those outcomes as proof of non-execution.

## Bounded recovery consumers and pacing

Checkpoint capture binds exact captured through/incarnation/source seal references;
retained export/rebuild processes one physical segment outside lifecycle
transactions. Maintenance, reconciliation and unpublished recovery use current
projection plus exact original operation/effect evidence. Missing/stale/ahead/
unsupported segmented proof fails closed. Legacy fallback remains bounded by the
old physical limit; segmented foreground code never falls back to lifetime scans.
Each consumer needs its own query/byte test and integration caller-inventory row.

Pacing is a separate single-purpose commit. Unchanged PublishConnection
`agent_parent_not_ready` read-only observations use the independently default-off
`CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED` flag. They reuse persisted
exponential delay capped at five minutes; existing `agent_runtime_unavailable`
pacing keeps its current configuration and behavior. Parent progress,
new authoritative work or stop/delete bypasses stale delay; lease replacement and
restart preserve fencing/attempt counts. Existing ledger-hold backoff is untouched.
Every actual attempt and effect receipt remains honest. Measure maximum wake delay;
pacing prevents growth but cannot recover the nine already-failed Agents.

## Operator and user behavior

Expose authorized dry-run, idempotent admission and operation status through the
existing lifecycle/receipt surface, with typed dispositions, current phase,
retryability and required next action. No private unauthenticated operator route.
The disabled path must emit no new format or work; production admission needs
separate reviewed cohort authorization after the writer canary.

Users retain the same Agent page, history and original responses. While blocked,
explain that history capacity recovery is waiting on the parent/runtime or exact
retained evidence. During admitted recovery show recovery in progress; do not
advertise Ready until confirmed. After recovery, original responses retain their
outcomes and new work uses ordinary command admission. Retirement stays visible
as retirement. Authorized status may expose its existing operation identity;
telemetry/issue comments contain fixed reasons and aggregate counts only.

## Compatibility, rollout and rollback

Migration identity: `2026-11-05-am-log-capacity-recovery-v1`, subject to existing
ledger/checksum verification. Additive schema/collection/grant/backup registrations
must follow S1/S4/S2 ownership; do not silently introduce a third gate/admission
collection. Missing/disabled `agent_log_capacity_recovery_v1` is closed. Explicit
cohort initially admits one operation; it is independent of writer percentages.

Before any adoption, deploy compatible recovery readers/workers and test the
actual permitted rollback executable. Drain incompatible writers and prove the
reader floor; v2 marker is a decode tripwire, not protection from in-flight stale
raw writers. New generation/fence and parent/Agent CAS reject old authorities.
After first adoption, rollback closes new admissions and uses a certified
segment-aware writer/recovery build. Keep accepted recovery, history and receipts;
never delete markers/seals, lower the head or restore an old snapshot over writes.

Production acceptance follows authorized writer canary and individual fresh
dry-run approval, then at least 36 minutes of read-only observation. Record all
nine current dispositions; parent prerequisites are not counted as restored
Agents. Release/activation is owned by the release lead, not this implementation.

## Tests and review gate

- Pure domain: exact capacity-only classification; parent Failed/Retiring/
  CredentialRotating; cleanup/terminal preservation; lineage/home/native changes;
  failure/head/revision mismatches; valid/expired/replaced leases; numeric bounds.
- Atomic Mongo: all-or-none adoption + successor + projections + recovery receipt;
  concurrent attempts; matching/conflicting key; later-head receipt reconciliation;
  failpoints before/after commit; no duplicate successor and no source mutation.
- Worker: crash at each durable boundary; same native/open identity; no Message,
  Steer, first-message or managed-context send; no acknowledgment before durable
  output; blocked/timeout behavior; owner/parent supersession; gate-close finish.
- Compatibility: old data/new code, default-off no emission, actual pre-format
  executable refusal, permitted recovery binary, complete original pages/SSE,
  unchanged ambiguous/terminal commands and exact global boundary.
- Recovery consumers: 1/10/100/1,000 sealed segments with fixed requested evidence;
  no lifetime scan under lease; measured rows/BSON bytes/query count/peak memory.
- Gates: fmt, warning-free clippy, targeted unit + disposable real-Mongo tests,
  repository file-size script plus every touched file under 500 lines, complete
  candidate-history secret scan. Heavy tests use heavy-0 and two Cargo jobs.

Plan targets: domain/dry-run contract Thu 06:00; complete recovery/checkpoint
candidate Thu 20:00; fixes Thu 23:00. Shared ports/registrations freeze Wed 21:00.
One planned S5 PR on the named branch, with pacing in its own commit/review section;
any additional PR needs the lead's recorded split. No PR is ready until modules
are registered and all required local gates pass.

## Initial domain implementation and integration handoff

The current implementation provides read-only preflight and a transient,
exact-source admission proposal. S2 registered it in the shared integration
branch at `b24da1bc8`. It exposes no public recovery route and performs no
database or runtime mutation. The executable S1/S4 contracts and remaining
store/worker/API integration are required before a review-ready recovery candidate. The proposal preserves Failed
status, conversation, command facts, receipts and global sequence; it only
advances the Agent revision/work fence and clears an expired work lease.


S2 owns `api/crates/layer2-core/src/sandbox/mod.rs` and added the following
registration in `b24da1bc8`, consumed by normal merge:

```rust
pub mod agent_capacity_recovery;
```

S5 implements the module and its child tests. Public names:
`CapacityRecoveryContext<'a>` (borrowed Agent, Sandbox, Workspace, Worktree),
`CapacityRecoveryDisposition`, `CapacityRecoveryContext::classify(now)`,
`CapacityRecoveryContext::plan(original_failure, at) -> CapacityRecoveryPlan`,
and `CapacityRecoveryPlan::fenced_agent(current_context, now) -> Agent`.
The transient plan preserves source/failure evidence and stable admitted time;
the exact fenced Agent proposal changes only revision, next fence, expired
lease and update time. No event or input state changes. S4 must commit it
atomically with durable non-ready recovery work and its source proof.
This internal preflight assumes the service already authorized a coherent
snapshot. It grants no admission or runtime permission. `EvidenceRequired`
means inspect the exact source and receipts; `ContinueOriginalWork` routes a
legitimate original publication to S4 without changing its input/effect key.
No `Eligible` result exists before source proof, gate and fence checks.

The first unit fixtures use real existing domain types and lifecycle fixture
builders. Preliminary tests used a temporary harness; registered core tests
now exercise the module directly. Provider allocation and native conversation
generations are independent pinned authorities. The original open receipt key
is `agent:<id>:open:v1`; the generic work-step name is not its effect identity.
Real-Mongo and runtime integration gates remain required.

## S1 captured evidence required by owned consumers

Each API below must accept the caller's existing `ClientSession`, bind the
exact Agent revision/head/incarnation in that session, and use format dispatch.
No hidden audit writes; missing segmented evidence fails closed.

| S5 caller | Required bounded fact | Preserved transaction fence |
| --- | --- | --- |
| `checkpoint_capture::validate_and_fence_begin` | Captured through plus exact newest event for activity; no lifetime visitor | Agent revision/head, Workspace and Sandbox capture fences |
| `checkpoints::agents::validate_and_fence` | Validated captured through for each live Agent | Agent revision/head checkpoint publication fence |
| `sandbox_maintenance::evidence::live_activities` | Exact newest event at captured through | Existing parent/Agent fences, pending command and open-session checks |
| `sandbox_reconciliation::agents::load_agents` | Bounded current semantic/projection evidence for every captured Agent | Existing snapshot decision and Agent revision/head fences |
| `sandbox_unpublished_recovery::transaction::load` | Bound proving maximum opened/retired runtime generation is below proposed generation, including absence proof | Every child plus Sandbox revision/history fence |
| Capacity preflight/admission | Exact original capacity failure, creation identity and native/open receipt at captured source | Current resource access, Agent, parents, native/home and work lease |

A latest-row query alone is insufficient for checkpoint/history integrity.
A find-one miss for a generation is insufficient without captured projection
coverage. Unpublished recovery needs an exact covered generation maximum/set
fact; it must not scan the lifetime history to prove absence.

## S4 atomic recovery contract required

S5 supplies an authorized exact capacity-failure source and stable original
failure/creation operation identity. S4 owns the sole publication transaction
helper and durable schema. That helper must support a storage-only admission:
seal original valid `[1,H]`, successor starts `H+1`, no synthetic events,
unchanged global head, independent writer/recovery gate CAS, immutable exact
receipt and the durable recovery work carrier in the same transaction.

The published S4 `PublicationInput` / `AggregateUpdate` interface supplies the
atomic storage boundary. S5 owns recovery work/runtime semantics and coordinates
its durable carrier with the shared schema/registration owners. The current
draft `Admission` has
no recovery phase/lease/native target payload; ordinary `AgentWorkStep` has no
recovery step. Reusing `PublishConnection` on a Failed Agent is not valid:
current validators reject it, and ordinary completion can schedule first-message
dispatch. The chosen carrier must keep the Agent non-ready until restricted
runtime confirmation, permit exact accepted continuation after gate close,
and fence old writer/runtime claims. S5 will implement the domain/worker
semantics in its assigned modules without inventing another gate collection.

Unknown commit must expose exact admission/publication receipts under a majority
barrier. Absence is unknown; later global head is compatible with exact success.
No recovery code will set a boolean claiming a successor exists independently
of the writer's transaction proof.

## Retry-pacing ownership gap found on current develop

Reuse the existing advisory `worker_failure_pacing_v1` mechanism: it already
persists repeats and parent/Agent/command generations, wakes on changed
parents/commands, caps at 300 seconds, and filters held work before LIMIT.
Adding a second durable pacing schema or raising `Agent.work.ready_at` alone
would lose those wake semantics.

The frozen S1/S4 handoff allocates these required files to S5:

- `api/crates/cma/src/store/agent_failure_pacing.rs`
- `api/crates/cma/src/store/agent_failure_pacing/discovery.rs`
- `api/crates/cma/src/sandbox/agent_worker/support/context.rs`
- `api/crates/cma/src/sandbox/agent_worker/connection.rs`

Implement only an
independently default-off `agent_parent_not_ready` read-only observation path;
retain current `agent_runtime_unavailable` baseline and ledger behavior. Capture
parent and command-admission revisions before the observation; persist only
matching read-only failures. The config/worker/store composition registration
belongs to S2. The frozen flag is
`CHRONO_AGENT_PARENT_NOT_READY_PACING_ENABLED=false`.
Current baseline runtime-unavailable hint behavior is already enabled; the new
flag must not silently rewrite that baseline under the label of default-off.

## Next gates

Normally merge exact isolated S1/S4 dependency heads and shared registration
commits; do not import the evolving integration tip. Finish store/worker/API
and consumer wiring against the frozen ports,
then exact-head fmt, warning-free clippy, domain/worker/real-Mongo tests,
under-500-line checks and complete-history secret scan. No PR is review-ready
while executable storage/runtime integration is absent; the domain handoff is
not a recovery delivery.


## State-machine source checkpoint

The registered core module now includes `ports`, `state`, and `service`.
The service is independently default-off. Its local storage seam carries an
opaque captured proof and explicit Recorded/KnownAbort/Unknown outcomes.
Its runtime seam exposes only original-target reconnect and acknowledgement;
there is no input, settings/context replay, replacement or new-conversation
method. These local ports do not replace S1 source proofs or S4 publications.

Accepted work keeps the original creation/failure key and target across retries.
Storage admission stays Failed with no ordinary work scheduled. Recovery claims
advance the writer fence, runtime confirmation is checked against the exact
native target and original receipts, and completion produces only its actual
Failed-to-live status event. Parent stop/delete/retirement wins. Lease validation
uses elapsed monotonic time across the reconnect await. Input hashes include the
separately serialized common first-message binding; no input text is copied into
the bounded recovery work carrier.

The transactional model stub covers restart, gate closure, both kinds of unknown
commit, lease takeover/expiry, a crash after native confirmation, known-aborted
completion, parent intent changing during reconnect, wrong native identity,
corrupt work, approval state and queued/terminal/uncertain first-message outcomes.
The source contains 34 registered recovery tests (17 earlier preflight tests and
17 new orchestration tests). This is test inventory, not a passing test claim.

Consumed dependencies: S1 `5ed7eaae1`, S4 `453b41742`, projection root registration
`5f20045a6` and reader root/schema registration `ec6002898`, all normal merges.
The first integrated core test attempt reached S4 projection registration and
failed because the S3 `UsageFold` derives and `validate_projection_identity`
export are still absent. No recovery tests executed in that attempt. S5 reported
the concrete compile dependency and continues source work; no review-ready PR
or successful Mongo recovery is claimed by this checkpoint.

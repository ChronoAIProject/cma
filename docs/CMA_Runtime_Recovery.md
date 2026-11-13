# Recovery after an unpublished runtime open

A native conversation may open successfully before its tool authority receipt
is saved. The runtime remains alive while the control plane cannot publish the
conversation. Treating every already-initialized response as either a successful
open or a reason to close the process is unsafe: the former invents authority,
and the latter can discard an unflushed conversation tail.

## Retained-history deadlines and retry continuity

Recovery `initialize` (with a retained conversation ID) and retained `session/load`
allow five minutes idle, renewed only by durable output ACK progress, with the
existing sixteen-minute absolute RPC bound. Legacy sockets without ACKs get a
fixed five minutes. This covers cold reopen, prepared tool rebinding, managed
context reload after close, and instructions recovery after native unsubscribe.
Fresh `initialize` keeps thirty seconds idle; `session/new` and ordinary control
RPCs keep thirty seconds. Existing shorter optional-probe bounds are unchanged.

Settings/context delivery normally has a fifteen-second idle budget, including
progress-subscription setup before enqueueing. Context delivery with durable
progress also has a 120-second absolute bound. Only an admitted retained load
switches delivery to its five-minute idle and sixteen-minute absolute bounds;
ordinary budgets restart after loading. Another operation's load/output cannot
extend a pending authority guard. This prevents an outer waiter from repeatedly
abandoning a closed session while its history loads.

The fifteen-minute lifecycle lease still bounds the whole sequential reopen
step. Socket loss stops its relay, while Layer 4 retains the process, in-flight
load and unacknowledged output. A full journal backpressures stdout until the
next attachment ACKs it. Retry reuses the bridge and completed loads; retained
process state does not prove that an unfinished load succeeded.

The shipped patched ACP 1.4.0 bridge dispatches requests concurrently. The pinned
Codex 0.147.0 regression receives `Already initialized` while a load is blocked
in required MCP initialization. Before session installation, both mode-setting
and prompts return JSON-RPC -32603 (`Session … not found`). The normal Codex
handshake requires the mode acknowledgement before publication. After installation,
prompts use that native session without waiting for ACP history replay. Required
tool completion receipts remain separate checks; `Greeting::Existing` alone is
not a load-completion receipt.

A prompt refusal becomes `FailedForPrompt`, durable `TurnFailed` with class
`agent_turn_failed`, and AG-UI `RUN_ERROR`. The Agent returns to Ready with its
active turn cleared; the failed input is neither reported complete nor resent.
A failed load still requires successful reload/recovery of the same conversation.
The native regression proves refusal during and after failed loading, then
successful explicit reload of the original ID.

Revive REST returns HTTP 202 with durable `reviving` state without awaiting the
worker. REST reads expose the recorded lifecycle purpose, phase and start time;
the AG-UI sandbox companion exposes status only. The Agent UI polls public REST,
shows “Reopening your agents”, and never advances phases from elapsed time.
Ordinary AG-UI input is disabled with a reason; Responses v2 can accept durably
queued input during revival. The caller therefore has an admitted in-progress
result while a worker exceeds 120 seconds, not a 120-second completion guarantee.

## Preventing and recovering lost confirmation

The open runner retains its original channel and immutable binding while retrying
only confirmation. Each source allows three attempts bounded by five seconds per
attempt and fifteen seconds overall. Changed authority stops the operation.
Provider transactions retry only definitively transient outcomes; unknown commit
outcomes require exact readback and never authorize a blind transaction restart.

New Codex runtime images expose the read-only completion protocol documented in
[CMA Open Completion Proof](CMA_Open_Completion_Proof.md). An initialized bridge
with a missing confirmation can be reused only when two challenge-bound receipts
prove the exact native conversation, descriptors and process/session generations
before and after applying settings. This does not prove optional tool readiness.
Unsupported or mismatched evidence remains a refusal.

Opening failures retain their original cause through reader teardown. A local
acknowledgement-channel closure must not replace a provider refusal in telemetry.

## Recovering an older failed runtime

The existing owner/operator runtime-update command can admit a narrowly scoped
recovery of a managed native home. It requires a complete Sandbox, Agent and
Workspace snapshot, original retained storage authority, original conversations,
and evidence that this runtime generation never published or accepted CMA work.
Healthy active generations cannot enter this path.

A revival or image rotation stuck at ConversationReopen may also qualify after
a persisted retry, between worker leases. This does not require a terminal update
failure or an elapsed-time threshold. The same complete child snapshot, full
publication logs and native proof must show that the retained generation never
published. An explicit owner/operator update can supersede an older queued target;
automatic recovery retains any queued or started target and requires it to equal
the currently released image.

First-message `dispatch_generation` records the Agent work-authority fence; it
does not identify a Sandbox runtime generation. A completed first message may
therefore have a larger fence than the failed runtime generation. Runtime-loss
recovery also deliberately removes process receipts, including OpenConversation;
the original conversation identity and complete durable publication history remain
authoritative. Every Agent's full log still vetoes recovery after any
ConversationOpened or RuntimeGenerationRetired at or beyond the source generation.
An unfinished delivery record or interrupted turn can be retained as history only
on a Deleted Agent with a valid deletion timestamp and no cleanup debt, pending
work, lease, blocking operation, queued message or outstanding stop intent. Those
records remain Deleted and are excluded from the conversations reopened by recovery.

The read-only provider inspection records a bounded, opaque plan with process
identities and history hashes. The Mongo admission transaction rechecks the full
snapshot, fences every child and original event head, and records the operation
before any process effect. Existing Agent records, queued messages, output
checkpoints and user files remain unchanged.

At the checkpoint-verification phase the worker executes the admitted provider
plan. The Codex adapter journals the operation on the original PV and fsyncs it
before signaling the exact, source-verified Node bridge. Closing its native stdin
allows the pinned native process to run its normal EOF shutdown. The adapter
never force-kills the native process or its process group after a drain timeout.

The provider verifies original history prefixes, final persisted turn evidence,
exact process disappearance and unchanged runtime/home identity. A retry uses
the same journal and refuses replacement PIDs, a different boot or changed source
authority. Native proof failures retain the old runtime and retry; they never
authorize suspension. Historical abandoned turn markers remain history and are
not rewritten into synthetic completion records.

The worker persists a `RuntimeRecoveryQuiesced` receipt and completion event in
one lease-fenced transaction before the existing home-verification path may
suspend the runtime. Replacement uses the original native Sandbox and home,
advances its generation, applies the admitted image and reopens the original
conversation IDs. A lost control-plane reply replays the durable provider journal.
Loss of a replacement runtime retains the recovery incident and can continue
through the normal retained-home lifecycle.

## Automatic recovery and operational limits

Background recovery applies only after ordinary retries have stalled at reopen,
failed or parked.
Its target must already equal the released managed image; it cannot silently
retarget an older admitted update. One automatic replacement attempt is allowed
per unresolved incident. Explicit owner/operator retry remains available, and a
successful publication clears the incident for a future distinct failure.
Candidate scheduling rotates so an unprovable earlier candidate cannot starve
other failed sandboxes.

A missing original volume, incomplete identity, unrecognized provider binary or
unprovable process state remains a refusal. No empty replacement volume or new
conversation substitutes for unavailable original data. Older API binaries that
cannot read the additive lifecycle metadata are not compatible rollback targets
after recovery has been admitted.

## Validation

Tests cover transient and unknown Mongo commits, stale child authority, duplicate
admission, exact receipt events, lost replies, PID/boot changes, history-prefix
retention, complete recovery and subsequent pause/update/retirement. Native
fixtures run the pinned Codex process with active work and a dirty workspace,
verify EOF shutdown, reopen the same conversation and send a subsequent prompt.
The Python process/journal tests run in Linux CI; GitHub CI builds and scans the
deployment images. Production recovery still requires checking the original
home, original conversation and resulting runtime independently for each sandbox.

## Recovery before any Agent exists

An original runtime that cannot run its supervisor uses a separate, read-only
provider proof. This path requires the complete Agent set to be empty, including
stopped and deleted records. A preparing Workspace may retain its pending Prepare
step; its identity, home and all stored bytes remain unchanged. A live-conversation
recovery continues to require the normal boot-bound history and graceful drain.

The provider inspects the exact original Claim, native Sandbox, PVC and owned Pod.
It verifies the original rendered request, image, mounts and completed initializers,
and requires the sole runtime container to be stopped after an error or waiting in
CrashLoopBackOff after that exact error. Running, ready, replaced, signaled or
ambiguous containers are refused. This inspection does not exec, bind, signal,
allocate or delete anything. The bounded opaque proof pins resource versions,
container identity, restart count and a digest of the complete Pod execution state.

A stalled RuntimeLost revival may retain generation 1 while generation 2 is only a
reserved attempt. An explicit update can supersede that revival only between worker
leases, after a retry, with the complete empty-Agent snapshot and exact provider
proof of the retained generation. Admission preserves the original binding, home,
preparing Workspaces and generation high-water mark. It replaces the old reservation
with a new lifecycle operation; the next allocation reserves above the old high-water
mark. It never rebinds the retained runtime to an invented generation.

The original provider object may already carry the exact reserved generation even
though runtime startup failed before a replacement binding was published. That
case requires a second complete provider proof rendered from the persisted
reservation, with the same retained image and specification. The reservation must
belong to the exact failed RuntimeLost operation, equal the durable high-water
mark and exceed the retained source generation. A provider annotation or high-water
number without that reservation grants no authority. The proof records the exact
reservation separately as optional `reserved_provision`; the original binding and
home-recovery source stay at generation 1. Older proofs without this field retain
their previous meaning. Recovery persists this evidence before replacing the old
operation, then uses its proved request for quiescence, suspension, lost-reply
verification and the later upgrade from provider generation 2 to newly reserved
generation 3. Every provider step still verifies the original Claim, native Sandbox
and PVC UIDs, home identity and complete admitted request. Unreserved generations,
changed operations, changed homes and request mismatches remain refusals.

A terminal RuntimeLost provisioning failure clears runnable work but retains its
exact failed operation, allocation reservation, owner, source binding and home
identity as `failed_provision`. Recovery may use this evidence only with a complete
empty Agent set, no worker lease, and fresh provider proof of the same nonexecuting
allocation. It admits a new operation; it never replays the failed operation or
lowers the generation fence. A failed snapshot survives API restarts and keeps the
same pending Workspace and files. Generic saves cannot add, replace or erase this
evidence; admitted recovery consumes it and explicit retirement may discard it.

Older Failed records that already lost their allocation reservation cannot use
that historical-reservation path. A separate explicit owner/operator recovery
requires a Failed provisioning record with its original suspended binding and
native home, no operation, lease or runnable work, a complete empty Agent set and
the last failed worker observation at least sixty seconds old. The high-water
mark bounds an independently verified current allocation; it never recreates an
operation or reservation. The adapter must render the same image/environment at
that generation and prove the exact live Claim/Sandbox/PVC graph, complete Pod
policy and request hash. Admission records `observed_provision_generation`
separately, preserves source binding and home, and allocates the replacement above
the retained ceiling. Missing, foreign or higher provider authority is refused.
Automatic recovery does not admit this legacy exception.

The sixty-second age guard covers the normal assignment call bounds: the adapter
wraps identity checks, exec delivery and acknowledgement in thirty seconds; the
remote assignment helper separately bounds stdin reading by ten seconds and socket
assignment by ten seconds. Dropping the exec client does **not** prove cancellation
of the remote process or undo an accepted assignment. Once accepted, supervisor
initialization may continue independently. These timeouts therefore reduce ordinary
request overlap; sixty seconds is not a process quiescence receipt and cannot rule
out arbitrarily delayed remote scheduling. The fresh complete process observations
and durable fences below remain mandatory, with their stated observation/CAS boundary.

For the pinned legacy runtime `sha256:3c5bccfabe8d20bd1fb5900cbfb2958878a316d75cfd1d186bcbfcf91c6fbf28`,
a read-only exec can additionally prove an idle assignment bootstrap after an
exit-2 startup failure. Pod Ready and bootstrap argv alone are insufficient:
assignment preserves argv. The script verifies the executable digest
`5a39d8c147fb9061e85831aa5fc7f42c405374719c4bc6d040e94dd2f997939b`,
PID/start identity, complete process and thread inventories, and every runtime
descriptor and socket. Only PID 1 and the inspecting process may exist. Only the
main and Tokio worker threads, standard pipes, reactor descriptors, internal
socket pair and exact assignment listener are permitted. The source revision
`4d1ed46a4ef4ebef6f12d927192151baa53613cf` starts a permanent child-reaper thread
before constructing the supervisor; its absence, along with the complete
descriptor proof, distinguishes the bootstrap from an assigned supervisor.
The image layer independently matches that binary digest. Other images are not
implicitly granted this capability.

The script repeats the inventory without sending an assignment, reading tenant
files, signaling, or modifying the home. The adapter rechecks Pod/container and
Sandbox identities around exec, then repeats the exact witness before the existing
Sandbox UID/resource-version suspension CAS. This is an observation plus the
durable no-work and empty-child fences, not an atomic process-freeze protocol.
Any new assignment, process, descriptor or identity visible at recheck refuses
recovery. The later suspension completion and retained-home proofs remain mandatory.

This recovery uses API code and the retained runtime image. Once admission persists
`reserved_provision`, `observed_provision_generation` or `failed_provision`, rollback must use an API binary that can read that evidence;
an older strict reader that rejects the optional field cannot safely resume the
in-flight recovery. Do not erase the evidence or lower the generation fence to
make an older binary accept the record.

The worker rechecks the full empty-Agent set and source proof, then commits a separate
quiescence receipt. Immediately before suspension it checks authority and the complete
Agent set again and refreshes the stopped-container witness while keeping the
admitted runtime, request and home identities fixed. The provider then rechecks that exact fresh
witness immediately before a suspension CAS using its observed Sandbox UID and
resource version. Running or changed evidence at that final boundary retains the
original runtime and home; a later safely stopped retry may refresh the witness.
A lost suspension reply is reconciled only by verifying the exact retained provider object is already suspended. No process command
or graceful-drain journal is needed for this branch. The lifecycle transaction also
refuses to advance this recovery if any Agent has appeared since admission.

After runtime recovery, the original Workspace Prepare executes under its original
operation identity. An incomplete staged clone without its completion marker keeps
its files and returns terminal `workspace_prepare_outcome_unknown`; the Workspace
worker records Failed and clears pending work, rather than retrying indefinitely.
The Sandbox remains Active. That incomplete Workspace requires explicit recovery;
it is not marked Ready and does not admit an Agent as a usable workspace.

## Runtime mutation ledger capacity

The API distinguishes a reachable runtime's full mutation ledger from runtime
loss, owner quota and native HTTP concurrency pressure. Both runtime transports
recognize only the exact pair `mutation_ledger_capacity` /
`runtime mutation ledger capacity is exhausted` before generic retryability.
Details are never trimmed or matched by prefix/suffix. For retained runtimes,
only these exact legacy code/detail pairs have the same meaning:

| Code | Detail |
| --- | --- |
| `mutation_authority_corrupt` | `runtime mutation authority contains too many records` |
| `mutation_authority_corrupt` | `mutation authority contains too many records` |
| `mutation_journal_corrupt` | `runtime mutation journal contains too many entries` |

Other corruption, stale/conflicting authority and prefix/suffix near-matches keep
their existing behavior. A capacity refusal is not evidence that an operation
completed or that a process, home or Worktree disappeared. It never authorizes
runtime replacement.

Held Agent startup, connection publication/recovery, first-message attachment,
skill installation and cleanup keep their original pending work. The existing
durable attempt count schedules retries at 30, 60, 120, 240 and then 300 seconds
from the completed failure. A configured base longer than 300 seconds remains
unchanged. Successful advancement resets attempts. Restart/takeover does not
reset the schedule. No lease is held while waiting; retries retain operation IDs
and request digests and claim normally allocated newer fences. Once capacity is
restored, recovery can be delayed by up to the current retry interval.

A fixed `agent_runtime_ledger_hold` failure records the reason. Consecutive holds
at the same step reuse the exact retained tail marker, with the full Agent,
lease, log and parent checks in the existing transaction; they do not append an
unbounded stream of duplicate diagnostics. At the normal Agent-log limit this
marker uses existing diagnostic headroom and preserves the active turn, partial
answer, output receipt and cleanup debt. If even that reserved headroom is
unavailable, the publication fails closed without settling the Agent as failed
or cleanup as complete; normal lease expiry controls takeover. This does not
repair a full Agent log or grant normal startup publication extra log capacity.

The API's structured operational metrics emit `agent runtime ledger hold
observed`, a fixed `reason=runtime_ledger_capacity`, and a saturating process-local
`failures_total`. It counts observed refused attempts, not Sandboxes, and resets
with the API process. It excludes provider error text, resource identifiers and
caller spans. `occupancy=unknown` and `occupancy_support=unknown` are explicit:
these failures provide no trustworthy occupancy/free-slot count. RuntimeStatus
JSON remains unchanged; observation does not scan or mutate runtime ledgers.

| API | Runtime | Capacity behavior |
| --- | --- | --- |
| Old | Old | Legacy nonretryable rejection; startup may fail and cleanup follows the old retry path. |
| New | Old | Exact legacy capacity pairs retain pending work with sparse retries. |
| New | New | The exact capacity code/detail pair retains pending work with sparse retries. |
| Old | New | Requires `mutation_ledger_capacity` with `retryable=false`; the old API treats it as nonretryable rejection, not runtime loss or quota. Startup can still fail. |

This is API containment only. Existing runtimes keep their image and ledgers;
retries can still consume authority on an old runtime, at a reduced rate. It
neither frees slots nor permanently bounds their growth. Runtime prevention and
any repair of an exhausted home require their separate release/recovery work.
No durable schema, operation identity, receipt or fence format changes here.
An API rollback can read these records but restores its previous failure/retry
behavior; format compatibility does not retain this containment policy.

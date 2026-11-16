# Sandbox conversation and workspace recovery failures

Runtime readiness, conversation attachment, and durable tool confirmation are
separate boundaries. A Ready Pod with its retained home can still have an
unfinished conversation reopen. A tool confirmation record is scoped to the
exact original Agent, request, runtime incarnation and conversation; an older
runtime's confirmation does not authorize the replacement.

## Access revoked during account deletion

`agent_access_revoked` and `workspace_access_revoked` are terminal operation
failure categories. Account deletion is in progress for an account required by
this operation, so CMA has revoked access. No Agent or Workspace recovery action
is available. These categories do not indicate a broken profile or runtime.
They preserve the deletion access boundary; the API does not offer an action
that restores access while the account is being deleted.

## Confirmation after successful native opening

The hosted adapter validates the returned launch and conversation receipt before
confirming each tool source. If confirmation is unavailable or its reply times
out, it retains the completed channel and retries that same confirmation. It
does not initialize the native bridge again, resolve a replacement attachment,
send a prompt, or publish the Agent as ready while confirmation is outstanding.

There are at most three attempts, each with a five-second deadline, within one
fifteen-second total budget per tool source. Brief backoff separates retries.
An immediate success has no extra wait. Refused or unsupported authority stops
immediately. Parent lifecycle/startup deadlines still bound the entire open.
Cancellation drops the current waiter and admits no detached retry task.

The source rechecks current authority on every attempt. Cancellation or timeout
does not establish that a database transaction aborted: a subsequent attempt
must recognize an exactly matching committed receipt without broadening it to a
different conversation, boot, request or authority. All required sources must
confirm before readiness can be published.

This prevents transient confirmation failures from unnecessarily abandoning a
successfully opened conversation. It does not manufacture evidence for an
already-initialized native bridge, recover an incomplete native setup, or
recover successful native setup evidence lost with the control-plane process.
Those cases retain their existing authority checks. Increasing retry counts or
copying an older receipt into a replacement runtime cannot repair them safely.

Access and provider authority failures record the operation, stage, fixed error
category and attempt number. Resolve and confirmation events include the Agent
identity directly, even when informational spans are disabled. Bearer-only
access authentication and setup events omit Agent identity. Credentials, raw
driver messages and stored records are never included in these diagnostics.

## Preserve the original opening error

Failed opening retires the unpublished bridge and cancels its readers without
waiting for socket cleanup. Startup-output recording still commits before
acknowledging output. When that acknowledgement or the update channel closes,
the drain returns an already-available opening failure in preference to the
local channel error. It does not wait for a still-running open, and a successful
open cannot conceal a failed output acknowledgement.

Only the provider's exact pinned already-initialized response selects retained
session recovery. Other initialization rejections retain their original class.
Neither change supplies missing tool confirmation or authorizes a different
conversation.

## Workspace preparation interrupted before completion

A started preparation journal can outlive a Git clone that never completed.
The private staging directory may then contain `.git` and owner data without
the CMA workspace identity marker, which is written only after Git succeeds.
During recovery of that exact journal-owned staging path, a proven missing
marker settles the existing indeterminate preparation outcome. It preserves all
staged files and leaves the workspace failed rather than exiting the whole
supervisor on every boot.

A completed staged workspace still requires its exact identity and resolvable
commit before publication. Wrong identities remain refused. Other I/O errors
remain retryable and block startup; this treatment does not make an unhealthy
filesystem ready. A missing marker in an already-published workspace retains its
separate refusal behavior. No marker is synthesized and no staging data is
deleted during this recovery.

An explicitly configured repository clone has a 600-second wall-clock budget,
including both Git transfer and checkout. The ordinary 120-second Git budget
still applies to other operations. A large repository can finish downloading its
pack but need more than 120 seconds to write its working tree; downloading a pack
alone is not a successful preparation. The longer clone budget does not alter the
requested branch, clone depth, request identity or completion checks. A clone
that reaches its limit still has its process group terminated and records a
failure; retained original and attempted checkout files are not deleted. An
explicit preparation-recovery retry remains a new recorded operation after the
previous terminal failure, never a manual journal or identity-marker rewrite.

## Unfinished history from a previous native process

A failed reopen can leave an old `task_started` record in a conversation that
the replacement native process loaded without running a new turn. Pinned Codex
0.147.0 leaves that record unchanged when the idle replacement exits through
stdin EOF. Waiting for that process to append a terminal event cannot resolve
the recovery after it has exited.

Final recovery verification accepts such a record only after the exact admitted
processes are absent. Its start must already occur in the admitted transcript
prefix, more than sixty seconds before the admitted native process started.
The transcript must retain its original file identity and byte prefix, and the
start's timestamp and ordinal must be unchanged. Current turns, missing clock
evidence, new starts, reused process IDs and surviving untracked processes
remain refused. Recovery neither writes terminal events nor changes native
conversation IDs.

These final checks derive the historical classification from the original plan,
including a journal admitted before that classification was available. The
plan, its digest, operation and database admission remain unchanged; the normal
worker records its verified completion receipt. The implementation is embedded
in the control-plane adapter and does not require a new retained runtime image.

## Failed conversation startup

Protected tool setup errors include only a fixed handshake phase, for example
`channel tool session setup did not complete (phase: session_new)`. The phase
distinguishes initialization, tool capability/confirmation, authentication,
session creation/loading, permission configuration and completion rechecking.
It contains no runtime error text, tool URL, descriptor or credential. The
generic prefix alone does not establish that a channel tool caused the failure.

An initialized bridge without the exact held conversation/tool confirmation is
not evidence of an unsupported capability. When a held identity and a supported
native completion contract exist, unavailable completion evidence remains
`agent_runtime_unavailable` and recovery retries the same exact proof. Otherwise
setup is `agent_runtime_rejected`: the current contract cannot discover a lost
initial conversation ID or manufacture confirmation safely. This explicit
terminal result avoids an endless startup retry. It sends no substitute session
or prompt. A fresh capability declaration that lacks required HTTP tool support
remains `agent_runtime_unsupported`.

A terminal failure at `open_conversation` or `publish_ready` sets the Agent to
`failed` and atomically settles its never-dispatched creation-time first message
as `failed`, code `agent_startup_failed`, with no turn sequence or dispatch
attempt. `retryable: false` means the failed Agent cannot dispatch this message by
replaying creation or input. Inspect the Agent's retained failure, repair the
dependency/configuration, and explicitly create a replacement when appropriate.
Creation replay returns the original failed Agent. Accepted Responses commands
keep their identities and settle through the command worker; unaccepted input
receives `agent_command_not_admissible`, not an instruction to retry forever.

Historical failed Agents may retain `first_message.state: queued`; this is not
active work when Agent status is terminal. No historical records are rewritten
by this fix. `recover-worktree` remains limited to its documented initial
AddWorktree failure; it cannot reopen a failed conversation. Ambiguous dispatched
input retains its separate recovery rules and is never relabelled undispatched.

Rollout must keep compatible readers after `agent_startup_failed` is persisted:
older first-message validators do not accept this undispatched failure shape.
The first rollout must prevent an older reader from serving these new records;
drain incompatible readers before the new worker publishes failures, or stage a
reader-compatible deployment first. Ordinary mixed-version rolling overlap is
not sufficient for that guarantee. Keep this reader when rolling back unrelated
behavior. Never clear durable input or its evidence to make an old binary accept
a record.

## Failed startup after a reserved runtime generation

A stalled RuntimeLost revival can retain its bound source at generation 1
while its exact pending provision reservation and provider object are at
generation 2. Empty-Agent recovery can prove that reserved provider request
separately without changing the original binding or home-recovery source. The
complete Agent set must be empty, including stopped and deleted records; the
failed operation, reservation, retained image, full request and original native
Claim/Sandbox/PVC home authority must all agree. A provider generation or durable
high-water number alone does not authorize recovery.

Admission stores the exact old reservation in the optional `reserved_provision`
proof field before replacing the failed operation. The worker uses that same
proved request to verify nonexecution, persist quiescence, suspend the original
provider object and reconcile a lost suspension reply. The next allocation is
reserved above the existing high-water mark: source 1 and proved reservation 2
therefore recover to generation 3 on the same home, using the admitted target
image. A lost upgrade reply replays that same transition. Preparing Workspaces
retain their original operation and files, and recovery creates no Agents.

A completed ephemeral diagnostic container does not by itself prevent the
nonexecution proof. Every ephemeral specification must have one matching status,
with no duplicate or extra names, no readiness, running/waiting or prior state,
no restart, and a completed terminal record with ordered start/finish timestamps
and matching nonempty container IDs. Missing or ambiguous evidence is refused.
The proof includes the complete Pod specification/status digest and exact Pod
UID/resourceVersion. The provider refreshes the proof immediately before the
Sandbox suspension compare-and-swap; any changed or running container prevents
that attempt from suspending the runtime.

## API promotion and rollback compatibility

The history verifier, native provider proof and reservation recovery worker are
control-plane code. An API-only promotion can replay an existing admitted
history journal and admit the reserved-generation recovery; installing a new
runtime image is not a prerequisite for these fixes. Each recovery still uses
its admitted runtime target and retains the original home.

API rollback must preserve the ability to read every persisted recovery proof.
New readers accept older proofs without `reserved_provision`; older readers
that reject unknown fields cannot read a proof containing that field. Admission
also writes the proof into the append-only `RuntimeRecoveryAdmitted` event, so
completion and removal of the current lifecycle state do not by themselves make
an older API compatible. Retain a compatible API reader after such admission;
an unchanged runtime image does not establish API rollback compatibility.

## Verification boundary

Regression fixtures cover ambiguous confirmation readback, changed authority,
bounded timeouts/cancellation, one native open with unchanged tool binding,
interrupted and completed staged preparation, wrong identities, and genuine
I/O errors. Previous-process recovery fixtures also cover admitted-journal
replay, exact history preservation, process absence and current-turn refusal.
A pinned native fixture creates an isolated crash, cold-loads its unmatched
start without model work, drains through EOF without rewriting it, and then
continues the original conversation through an explicit local fixture prompt.
Reserved-generation worker fixtures preserve source generation 1, prove only its
exact operation reservation 2, and replay lost suspension and upgrade replies to
generation 3 with the original home and pending Workspace unchanged. A provider
at unreserved generation 3 is refused without suspension or allocation. Separate
provider fixtures cover complete terminated ephemeral status and rejection of
missing, changed, running or ambiguous records.
Local fixtures do not establish that an existing production sandbox
has recovered; production acceptance must additionally confirm its original
home, conversation and admitted work after the corrected lifecycle completes.

## Never-ready Agent worktree recovery

A large Git worktree checkout previously shared the ordinary 120-second Git
command deadline. It now shares the bounded 600-second checkout deadline with
repository cloning. Git queries and cleanup retain their 120-second deadline.
A timeout still fails closed if Git leaves a partial path or branch; CMA does
not roll back an indeterminate runtime journal or overwrite the partial files.

The owner or a CMA administrator can explicitly call
`POST /api/v1/agents/{id}/recover-worktree` with an `Idempotency-Key`. The endpoint
admits only a never-ready failed manual or Workspace-profile Agent with the exact
indeterminate AddWorktree failure, matching cleanup debt, and no process, home,
conversation, first-message dispatch, stop or deletion effects. The Sandbox must
be active and idle, and its existing Workspace must be ready. The transaction
reserves the next worktree name under the same Workspace and PV and restarts the
same Agent ID. It preserves the pending first message, profile, creation
provenance and every existing log entry.

The failed worktree's path, branch, base commit and runtime mutation journal stay
untouched. Its database assignment is detached from the Agent; its full original
record, failure and exact cleanup debt are retained in an immutable recovery
archive on that Agent. Ordinary Agent saves and worker publication cannot alter
this archive or rebind the worktree. A matching retry returns the current Agent,
including after deletion, and never allocates another worktree or resurrects it.
Each new attempt after another qualifying terminal failure requires a new key;
there are at most 64 archived attempts. Parent, quota, allocation and Agent
fences are checked in the same transaction. Uncertain Mongo commits require
exact durable attempt evidence before success can be reported.

Deploy compatible API readers before admitting recovery, and retain compatible
rollback readers after the first archive is written. If the failed checkout was
caused by the old deadline, install the corrected managed runtime through the
existing Sandbox lifecycle before recovering the Agent; preserve its existing
home/PV. The recovery API itself does not replace the runtime or mutate
agent-sandbox resources. Production verification must confirm the original PV,
retained failed files and journal, same Agent ID, successful successor checkout,
and a usable conversation before declaring the affected Agent recovered.

## Stop scheduling after process publication

A stop request fences the previous Agent worker and sends a serialized,
durably fenced CloseAgent operation. Once an exact OpenConversation receipt and
conversation identity are recorded, publication, connection, first-message and
idle work without outstanding cleanup debt no longer inherit a future connection
lease as a shutdown delay. This prevents an old 18-minute lease from delaying
Channel retirement or an owner stop. In-flight filesystem startup and skill work
retain their cleanup drain boundary. Runtime cleanup remains authoritative:
terminal state is published only after the close operation succeeds, and the
runtime rejects a late OpenAgent after its process has been retired. The change
does not delete the user's home, worktree or conversation history.

# Sandbox conversation and workspace recovery failures

Runtime readiness, conversation attachment, and durable tool confirmation are
separate boundaries. A Ready Pod with its retained home can still have an
unfinished conversation reopen. A tool confirmation record is scoped to the
exact original Agent, request, runtime incarnation and conversation; an older
runtime's confirmation does not authorize the replacement.

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

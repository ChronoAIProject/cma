# Durable original-conversation startup replay

An original conversation load can emit more output than the runtime's retained
window before returning its response. Previously, the adapter withheld all
receipts until the handshake completed, and lifecycle recovery did not consume
updates until reconnect returned. The runtime exhausted either its 8 MiB byte
window or its 16,384-frame window, permanently invalidating replay.

Quiet recovery now exposes receipts for the caller's exact stored native
conversation after the runtime's Agent declaration is checked. The lifecycle
worker consumes those receipts while reconnect and settings are pending. Each
receipt is recorded in the existing private Agent message checkpoint before its
ACK is sent. The transaction checks the original conversation, quiet Paused
Agent, Agent fence, replacement runtime generation, exact current lifecycle
lease and conversation-reopen phase. Mongo writes the parent inside that same
transaction so lease takeover or a phase change conflicts with a stale writer.

The replacement checkpoint may precede publication of the Agent's replacement
generation. The Agent remains Paused with its old published generation until
reconnect succeeds and the lifecycle transaction publishes the complete set.
Old-generation recording cannot overwrite a replacement checkpoint. Final
publication preserves the exact stream/cursor receipt and its log anchor.
Historical output from a quiet load follows the existing recorder rules: it
advances the durable receipt without inventing a new turn or duplicating saved
messages. Conversation metadata is folded into the existing semantic log.

The runtime keeps the same retention limits and pauses reading native stdout
when its journal is full. Only cumulative durable ACKs release that prefix.
Explicit output gaps, unreadable frames and oversized native frames remain
permanent failures. Legacy output remains legacy after selection and cannot
upgrade to acknowledged replay. Retiring a detached bridge cancels its blocked
stdout reader; attached consumers retain the bridge long enough to drain final
output after process exit. Cancelling an adapter open aborts its reader and relay
tasks.

Recovery initialize and retained-history load have a five-minute idle deadline
that advances only on successful cumulative ACK progress, plus a sixteen-minute
absolute per-call bound. Fresh initialize keeps its thirty-second idle window;
new-conversation and ordinary control RPCs keep thirty seconds. Producer activity
alone does not extend these windows. Settings/context delivery follows the longer
window only while its retained load is active. Lifecycle draining remains bounded
by the current lease expiry, shared across sequential Agent opens. The runtime
keeps bridge processes and unacknowledged output across socket loss, allowing
retry to rejoin completed loads and drain retained replay. The pinned bridge
dispatches initialize concurrently with load; an already-initialized response
does not prove load completion. See
[Runtime Recovery](CMA_Runtime_Recovery.md#retained-history-deadlines-and-retry-continuity)
for the delivery bounds and limits of retry continuity.

## Already-invalidated output journals

This change prevents capacity exhaustion; it does not heal an existing lost
journal. ACKing its prefix, reconnecting, changing the starting cursor or waiting
cannot clear that condition. No repair was executed while developing this fix.

The existing `CloseAgent` operation is terminal cleanup, not a bridge recycle.
Its durable receipt can replay without terminating a newer process, and its
mutation authority retires the Agent slot for the current runtime generation.
The runtime regression `replacement_reopens_agent_process_while_preserving_filesystem_receipts`
explicitly refuses a subsequent `OpenAgent` in that same generation. Do not use
close/open, delete journal files, clear the lost flag, downgrade the output lane,
or start a different native conversation as a repair.

A supported repair proposal requires a separately admitted replacement runtime
generation on the exact retained home. Before admission, re-read the current
owner, Sandbox and lifecycle authority, runtime/Pod identity, original
claim/member/PVC/PV identities, and every affected live Agent. In one fenced
admission, prove that no Agent has an active turn, unsettled operation, approval,
lease, pending dispatch, or admitted input that could be interrupted; preserve
queued work for normal lifecycle settlement. A previous Paused/Ready observation
is insufficient evidence for execution.

The current incident is already in conversation-reopen. It must not be advanced
by editing status or generation fields directly. A separately reviewed recovery
operation must first supersede that exact lifecycle lease and durably record its
new target, then use the existing retained-home suspend/rebind/reopen sequence.
The target must include the prevention fix. The operation must retain the
original Sandbox/Workspace/Worktree/Agent IDs, configuration, home graph and each
original native conversation. If other Agents on the runtime are active, this
whole-runtime proposal is refused and a new narrowly fenced per-bridge recycle
capability would need implementation and validation instead.

Verify the new runtime incarnation on the original PVC, original native
conversation identities, contiguous durable startup receipts, successful final
lifecycle publication and unchanged saved history. Resume queued work only
through normal durable settlement. Keep old/new runtime identities and phase
receipts as evidence; do not claim uninterrupted runtime continuity after an
observed replacement. No live repair commands are supplied by this document.

## Deployment and rollback boundary

The checkpoint encoding is unchanged. The new writer nevertheless introduces a
semantic state that older API binaries do not understand: a replacement-generation
checkpoint alongside a still-Paused Agent's older published generation. Do not
run an older recorder concurrently for that recovering Agent, or roll back to it
while such provisional receipts remain. Complete the fenced reopen with the new
writer first. The database transaction fence protects current implementations;
it cannot add checks to an older process that does not participate in it.

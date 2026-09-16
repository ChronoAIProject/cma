# Channel reply tools

A CMA Agent created by a channel bot sends messages through an explicit tool
call. It can acknowledge a request, report progress, ask a question, and send a
result while its turn is still running. CMA does not automatically forward its
final answer. Ordinary manual and Trigger sessions do not receive this tool;
connecting an existing manually created Agent to a bridge does not change its
creation provenance or make it eligible.

Pending tool approval remains an authenticated CMA dashboard/API decision.
An active channel-created Agent may explicitly explain that policy with this
reply tool. Once approval projects WaitingAction, the current reply admission
requires InProgress and the runtime may already be blocked, so an external
waiting notice is not guaranteed. CMA's durable waiting status is the fallback.
The [disabled Channel control foundation](CHANNEL_CONTROLS.md) adds no automatic
final, progress, waiting or refusal sends and cannot approve a tool.

The return path is CMA's reply outbox → NyxID
`POST /api/v1/channel-relay/reply` → the original bot/chat/thread. The original
NyxID message identity remains authoritative. The model never selects a bot,
recipient, route, provider endpoint, or credential, and there is no direct
Telegram or other platform fallback.

The reply tool is a CMA-hosted HTTP MCP connection. It does not invoke the
`nyxid` CLI installed in the Sandbox. The CLI uses the Sandbox's scoped NyxID
credentials for connected-service work; Channel replies use the configured
Bridge's retained bot/channel authority. The external sender need not be that
bot's NyxID owner. CMA authorizes each reply against its original Channel input
and owns the durable outbox and provider delivery. A future CLI interface would
need to enter this same admission path and preserve its session eligibility,
destination binding and retry semantics.

## Runtime contract

`POST /api/v1/channel-tools` implements stateless streamable HTTP MCP with JSON
responses. Its independent CMA capability cannot authenticate ordinary REST
requests. Browser cookies and NyxID keys cannot authenticate this endpoint.
It supports `initialize`, `ping`, `tools/list`, and `tools/call`; notifications
are acknowledged without executing sends. Protocol versions are `2025-11-25`,
`2025-06-18`, and `2025-03-26`. GET/SSE subscriptions are not required or offered.

The runtime receives the endpoint and bearer through ACP `session/new` or
`session/load` MCP configuration after proving channel-created provenance.
The connection is omitted for other sessions. The runtime must advertise HTTP
MCP support. Tool configuration and secrets are excluded from retained launch
wire payloads and runtime home files.

Discovery is allowed while the conversation is opening. Reply execution also
requires a successful durable tool-open receipt, the exact currently published
runtime/conversation, and an authoritative active channel command. A capability
cannot create that authority. A process reconnect can reuse existing tool
configuration only with the retained successful receipt for that incarnation.

The CMA capability is encrypted in control-plane storage. It rotates when a
higher runtime generation replaces the current one. Its invocation namespace
remains attached to the same immutable Agent creation, so a retry after runtime
recovery retains its original reply receipt.

## Tools

`read_channel_context` accepts no arguments and returns the nonsecret
`reply_context` for the exact current channel input, or null when no eligible
turn is active. It does not send a message. A context that becomes stale cannot
authorize a new reply to a later input.

`read_channel_control_context` also accepts no arguments. It returns a retained,
random `source_control_reference` only for an eligible current channel-created
source with separately admitted sender-control consent. The reference is a
nonsecret lookup identity, never a bearer permission. With control admission
enabled and both operations supported by the exact current local runtime, the
result has this shape:

```json
{
  "source_control_reference": "ccr_0123456789abcdef0123456789abcdef",
  "operations": ["cancel", "steer"],
  "approval_mode": "dashboard_only",
  "availability": "available"
}
```

The operation list is the intersection of current Agent capabilities and the
exact published runtime attachment's capabilities. Cancel requires Interrupt;
steer requires Steer, and both require Conversation. Disabled fresh control
admission, a missing local runtime, stale runtime generation or mismatched native
conversation yields an empty list and `availability: "execution_unavailable"`
while retaining an otherwise eligible source reference. A runtime supporting
only one operation lists only that operation. Missing current source or consent
returns a null reference and `availability: "unavailable"`. Availability is an
observation, and every subsequent control request rechecks source authority,
consent and the runtime before admission and again before execution. There is no
remote owner proxy. The tool accepts no source, destination, reference or
authority arguments and sends nothing. It does not change `read_channel_context`
or `reply_to_channel`.

The mapping retains the exact source command, response and turn, original
sender/chat/thread, Bridge/bot/platform, Agent creation, allocation generation,
and complete consent receipt and digest. Concurrent reads and process recovery
reuse the same mapping. Revoking and re-enabling consent creates a new mapping;
the old one stays immutable and cannot restore the earlier consent. Source,
runtime, current grant/custody and consent fences apply before returning the
reference. An allocation reset cannot select its replacement through an old
reference. References are not advertised for a completed or approval-blocked
turn. No automatic send accompanies issuance, refusal or approval waiting.
This foundation requires the retained channel-derived invoke grant and its
exact allocation basis. A channel-created Agent reached through an
`ExistingAgent` grant without that basis receives no control reference. Its
existing reply-tool eligibility is unchanged.

`reply_to_channel` requires these three arguments and rejects additional fields:

| Argument | Meaning |
| --- | --- |
| `reply_context` | Exact current reference returned by `read_channel_context` |
| `request_key` | Unique key for one intentional message; unchanged on retries |
| `text` | Nonempty message text, at most 1 MiB in UTF-8 |

For example, the Agent reads its current context and uses it to send an
acknowledgement with request key `ack-1`. While continuing its work, it can use
the same still-current context and a different key, `progress-1`, to report
progress. Neither send waits for the turn's final answer.

JSON-RPC request IDs are transport correlation identifiers and are never used
as durable send keys. Retrying the same `request_key` and text returns the
original receipt, including after that turn has finished. Reusing a key with
different text conflicts. Receipt recovery precedes current-turn admission, so
a retry cannot be silently routed to a newer input.

The safe tool result includes a receipt ID, opaque `outbox_ids` for each prepared
message, status, duplicate-risk indicator, and typed failure when present. `queued` means durable admission, not confirmed
platform delivery. Multiple calls append atomically to the original response's
delivery order. Chunking retains that order. Unknown network/send outcomes are
recorded for explicit recovery and are never automatically retried.

## Explicit Lark/Feishu reply edits

`edit_channel_reply` replaces one of the Agent's own confirmed Lark/Feishu text
replies when the Agent intentionally calls it. It accepts exactly four arguments:

| Argument | Meaning |
| --- | --- |
| `reply_context` | Exact current reference returned by `read_channel_context` |
| `original_outbox_id` | One opaque `bro_…` ID from the original reply receipt's `outbox_ids` |
| `request_key` | New key for this intentional edit; unchanged on retries |
| `text` | Nonempty replacement fitting one prepared 1,900 UTF-16-unit message |

For example, after `progress-1` reaches `sent`, the Agent may select its first
`outbox_ids` entry and call `edit_channel_reply` with key `progress-1-correction`.
Each original chunk has a separate opaque ID. An edit changes only the selected
message and cannot split the replacement into more chunks.

The original must be an initial reply with a confirmed Sent receipt containing
its platform message ID, from this exact still-active input and the same immutable
Channel-created Agent session. Another input, an edit item, a pending or unknown
reply, and other platforms are rejected. Caller-supplied provider message IDs,
recipients and credentials are never accepted. Durable lookup binds the original
source, bot and destination, and rechecks them before delivery through NyxID
`POST /api/v1/channel-relay/reply/update` using the configured channel authority.
The acknowledgement must identify that same original platform message.

Edits append to the same response delivery order and use the same bot pacing and
current authority checks as initial replies. Multiple deliberate edits may target
the same initial reply. Retrying the same key, original ID and text recovers the
original edit receipt, including after the turn ends. Changing any of those values
under an existing key conflicts. Reply and edit keys share one namespace.
Unknown outcomes are retained without resend and block later output from passing
them; changing the key cannot bypass that order. There is no automatic progress
replacement or final-answer edit. New edit admission requires the active turn,
while a queued edit may finish under the original retained authority afterward.

The edit path is covered by controlled NyxID HTTP fixtures and disposable Mongo
Channel-session tests. All platform `live_verified` values remain false until
separate real-provider canaries establish them.

## Pending reply backlog

Each Bridge can retain at most 256 outstanding prepared messages across all
inputs, Agents, runtime incarnations and configuration generations. Every chunk
and explicit edit occupies one slot. Pending, claimed, sending, retry-wait and
unknown deliveries count toward the same cap. Sent, rejected and explicitly
abandoned outcomes release their slots while retaining their original history.
This fixed deployment bound adds no Bridge setting or new provider permission.

New reply/edit admission reserves all needed slots atomically under the existing
Bridge fence. A message requiring more free slots is refused in full with MCP
code `reply_backlog_full`; it creates no invocation, chunk or response-head
progress. Exact retries recover their existing receipts before capacity checks,
including when the queue is full or the original turn has finished. A capacity
refusal confirms that this call admitted no output; it is distinct from an
uncertain commit that must be recovered with the same key and content.

Explicit management retry of a rejected delivery reacquires one slot and returns
HTTP 409 `bridge_delivery_capacity` when full. Retrying an unknown delivery still
requires duplicate-risk acceptance and keeps its existing occupied slot. The
management command's exact replay recovers its prior receipt at capacity.
Automatic delivery retries retain their existing slots. Configuration changes,
key rotation and allocation reset do not reset the outstanding count.

Existing backlogs above the cap remain retained and recoverable; new admissions
wait for sufficient terminal outcomes. The cap requires every admission writer
to run this implementation. Older mixed-version writers do not enforce it, so
this remains part of the existing continuity deployment gate.

## Send pacing and provider cooldowns

The durable outbox shares one active send slot per NyxID provider, platform,
and actual bot across CMA replicas. Changing keys, routes, or Bridge
configuration does not reset that slot. CMA currently enforces a conservative
minimum interval of three seconds for Telegram and one second for Discord,
Lark, and Feishu. These limits pace separate tool calls while the Agent keeps
working; delivery does not depend on turn completion.

A provider 429 records an unknown send outcome and retains `Retry-After` as a
cooldown for subsequent messages. Numeric seconds and HTTP dates are supported;
missing or ambiguous metadata uses a sixty-second cooldown. Valid longer waits
are preserved. A late 429 also extends the cooldown after worker recovery,
without replacing a newer reservation or changing the original recovery status.
Waiting for a bot slot consumes no delivery attempt. Only a refusal before any
provider request bytes are written can enter a safe automatic retry state.

## Paused sessions and command order

An admitted channel message keeps its place in the shared Agent queue while
execution is held. Later normal UI or channel messages cannot overtake it.
Steer and interrupt controls can still address the captured active turn.

A released message at the head of a paused channel-created session's queue can
schedule revival under two independent checks: the current Channel execution
grant and the original human's lifecycle grant. CMA fences both grants, the
exact unchanged command, its queue position, and the original resource in one
transaction. The original command receipt remains unchanged; scheduling does
not send a prompt. After the lifecycle worker restores the original Agent and
conversation, the common command worker dispatches that queued message.

The lifecycle worker checks the original human grant before runtime restoration
and every advancing phase. It replaces the scoped runtime key through retained
mint, delivery, and revocation receipts. An uncertain mint remains held; it
never causes an automatic second mint request. Normal reply-tool eligibility
still depends on the Agent's immutable channel-created origin.

A channel-created child under a shared manual or Trigger parent follows the
ordinary parent's lifecycle and credential worker. Its queued message can
schedule revival only with the current Channel execution grant and the retained
configuring human's current permission to manage that exact parent. Scheduling
preserves the original queued command, child conversation and unrelated siblings.
Reusing the parent does not change its origin or grant reply tools to existing
manual or Trigger Agents.

An explicit partition reset preserves already accepted commands and replies on
their original Agent and conversation. Execution proves the exact retained
allocation and every successor through the current partition generation, while
checking the original grant and current Bridge policy. The reset does not grant
new admission to the retired allocation. Replies queued before the reset and
replies explicitly sent during the retained turn keep the original NyxID message
and destination; their delivery may finish after the turn completes. Revocation
or suspension of the original authority still prevents sending.

Stopping that Agent prevents further execution and new reply calls. An already
queued reply may still be delivered while the original resource records and
authority remain available. Deleted Agents, removed children and retired or
purged parents require separate cleanup evidence and remain ineligible for this
delivery path. A refusal before sending releases the exact claim and bot slot
with a retry delay; an uncertain Sending commit keeps its recovery boundary.

## Compatibility and implementation status

The stored reply policy retains the legacy spelling `final_response` for
compatibility. Its behavior is explicit tool delivery only. `disabled` prevents
reply admission. `final_response_and_approvals` remains unavailable until the
separate verified approval-control contract is implemented; it does not enable
automatic answer forwarding.

The tool runtime connection, HTTP adapter, encrypted capability storage,
durable outbox, and bounded delivery workers are composed with channel-created
resource provisioning and the common Agent command worker. Disposable
HTTP/Mongo integration tests exercise signed callback admission, actual
channel-created sessions, multiple replies before turn completion, receipt
recovery, grant revocation, and the absence of automatic final-answer sends.

These tests establish the CMA integration using controlled provider fixtures.
Live Telegram delivery and production upgrade acceptance remain pending.
Uncertain-mint cleanup retains the original request plan, identifies an exact
orphan through the original owner's key inventory, and records that identity
before scoped revocation. A fresh complete original-owner absence check is
required when NyxID returns 404; that response alone does not prove revocation.
Cleanup never issues a second mint request or turns an uncertain mint into a
usable credential.

Channel retirement has a dedicated original-human authority path for a
completed, exactly owned resource tree with a proven runtime binding. It records
Agent stop, credential revocation and runtime teardown progress durably. Fresh
allocations with complete runtime and original PVC acknowledgements can finish
namespaced cleanup and publish Retired with sealed, retained history using
existing permissions. Missing or ambiguous original receipts remain held;
history receives no purge deadline, and underlying disk reclamation belongs to
the storage provider. Shared-child and owned-Workspace cleanup use their own
explicit consent and exact allocation ownership paths, while image rotation
remains unfinished. See [Channel lifecycle recovery](CMA_Channel_Lifecycle_Recovery.md)
for these boundaries. Application deployment alone does not enable Channel
traffic; the independent rollout selectors still govern admission and effects.

# Channel reply tools

A CMA Agent created by a channel bot sends messages through an explicit tool
call. It can acknowledge a request, report progress, ask a question, and send a
result while its turn is still running. CMA does not automatically forward its
final answer. Ordinary manual and Trigger sessions do not receive this tool;
connecting an existing manually created Agent to a bridge does not change its
creation provenance or make it eligible.

The return path is CMA's reply outbox → NyxID
`POST /api/v1/channel-relay/reply` → the original bot/chat/thread. The original
NyxID message identity remains authoritative. The model never selects a bot,
recipient, route, provider endpoint, or credential, and there is no direct
Telegram or other platform fallback.

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

The safe tool result includes a receipt ID, status, duplicate-risk indicator,
and typed failure when present. `queued` means durable admission, not confirmed
platform delivery. Multiple calls append atomically to the original response's
delivery order. Chunking retains that order. Unknown network/send outcomes are
recorded for explicit recovery and are never automatically retried.

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
Runtime credential renewal and other milestone 17 lifecycle work also remain
in progress. The current milestone work has not been rolled out to production.

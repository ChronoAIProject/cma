# MCP Interface

The general public `POST /api/v1/mcp` interface remains unsupported and returns
`501 application/problem+json` with `code: "interface_not_available"`,
`interface: "mcp"` and `Cache-Control: no-store`. Use [REST](API_REST.md),
[AG-UI](API_AGUI.md) and [Responses](API_Responses.md) for public resource and
conversation operations.

CMA also supplies each authenticated runtime Agent a limited MCP capability at
`POST /api/v1/agent-tools`. That capability exposes only
`agent_key_scope_revision`, which records a request for owner approval. The tool
cannot approve access. Its `request_key`, `services` and `reason` identify an
immutable attempt; an optional `retry_of` links a new attempt to a failed request
from the same Agent creation and original conversation.

Reuse an attempt's exact body and key after an uncertain transport result or to
observe its current status. A `failed` result stays failed for that key. It
includes a safe `error` and a `next_request` containing a fresh deterministic
`request_key` and its services and reason. `next_request_retry_of` identifies the
failed predecessor: include it as `retry_of` when the tool schema supports that
optional input. A retained conversation with the original cached schema can
submit the three-field `next_request` without restarting. When the owner asks to
try again, call the tool with that new proposal. Reuse the new
key for subsequent transport retries. The new attempt starts `pending`, retains
the failed record, and requires fresh owner approval. It inherits no approved
scope, credential, lifecycle operation or notification from the failed attempt.
Only one successor may refer to a failed request; if that successor also fails,
its own `next_request` starts the next attempt. Active, denied and succeeded
requests cannot be used as retry predecessors.

`pending` awaits owner consent; `connecting` and `applying` require observation.
Only `succeeded` confirms that CMA verified the runtime access update. A denied
request never includes an automatic retry instruction. GitHub access uses the
canonical service slug `api-github` or `api-github-pat`, selected for the owner's
connection type; `github` is not a registered service slug.

The owner-facing access-request list includes optional `retry_of` so clients can
associate a new consent request with the preserved failed attempt. Approval and
denial remain explicit public owner operations; a tool retry never acts as
approval or silently restarts another Agent or Sandbox.

After a linked retry is first recorded, rollback must retain a reader that
understands the optional persisted `proposal.retry_of` field. Earlier binaries
with strict proposal decoding cannot read those new records.

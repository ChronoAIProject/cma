# MCP Interface

The general public `POST /api/v1/mcp` interface remains unsupported and returns
`501 application/problem+json` with `code: "interface_not_available"`,
`interface: "mcp"` and `Cache-Control: no-store`. Use [REST](API_REST.md),
[AG-UI](API_AGUI.md) and [Responses](API_Responses.md) for public resource and
conversation operations.

CMA also supplies each authenticated runtime Agent a limited MCP capability at
`POST /api/v1/agent-tools`. That capability exposes
`agent_key_scope_revision`, which records a request for owner approval, and the
bounded document tools below when durable document storage is configured. The access tool
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
Only `succeeded` confirms that CMA delivered the new key to the retained runtime,
revoked the old key, and confirmed delivery of the callback to the original
Agent conversation. An active turn receives durable steering so notification
does not wait behind that turn. The model's response may finish later. Scope
updates preserve the live Sandbox, Pod and conversations. Optional services can
also be removed through the owner's public service-access Save; all Agents in
the Sandbox share the resulting key and the required platform baseline remains
selected. A denied request never includes an automatic retry instruction.

The runtime's genuine `gh` CLI already routes GitHub API requests through CMA's
`api-github` (GitHub OAuth) service. Request `api-github` when that access is
missing and wait for CMA's confirmed update; no Sandbox restart or personal
secret is needed. `github` is not a registered service slug. Other services,
including `api-github-pat`, remain independent requestable capabilities; granting
`api-github-pat` alone does not configure this runtime's `gh` route.

This API connection does not authenticate native `git clone`, `git fetch` or
`git push`. An optional repository is passed to the Agent for setup using its
available tools; CMA does not clone it on the Agent's behalf. If setup requires
unavailable Git transport, the Agent must report that limitation and preserve
the workspace rather than request a secret or restart the Sandbox.

The owner-facing access-request list includes optional `retry_of` so clients can
associate a new consent request with the preserved failed attempt. Approval and
denial remain explicit public owner operations; a tool retry never acts as
approval or silently restarts another Agent or Sandbox.

After a linked retry is first recorded, rollback must retain a reader that
understands the optional persisted `proposal.retry_of` field. Earlier binaries
with strict proposal decoding cannot read those new records.

Hot scope writes add optional exact `service_scope` credential revisions,
`scope_mode`, `expected_scope_revision`, `progress.hot_update` and durable
notification intent, plus the `updating` workflow phase and
`ServiceAccessRevised` Sandbox event. Once any of these are persisted, every API
reader and rollback image must retain the compatible readers. Reverting to an
older binary with strict decoding is not a supported rollback. Historical
additive grants and already-paused access workflows remain readable.


## Document publication

`publish_document` accepts `{request_key, title, markdown}` and creates a private
document in the authenticated Agent owner's personal registry. `update_document`
accepts `{request_key, document_id, expected_revision, title?, markdown?}` and
revises only a document previously authored by that exact Agent. The runtime
cannot select an owner, upload destination, share grant or comment authority.
The owner can independently read and edit these documents through public REST
and the Artifacts UI, even after deleting the originating Sandbox.

A confirmed tool result includes the durable document metadata and an
`/artifacts/{document_id}` link. Return that link to the user. A storage failure
is an explicit tool error; retry its exact key and body without claiming a save.
A stale expected revision preserves the owner's newer work. All metadata commits
recheck the full current Agent, runtime and conversation identity after object
upload. User-supplied REST origin labels cannot forge runtime write authority.

Sources are at most 1 MiB UTF-8 Markdown; the JSON envelope accommodates escaped
text. Other tool requests retain the original 16 KiB bound. All tools share the
existing rate, concurrency and request deadlines. Machine storage credentials
are confined to the control plane. They are never the runtime tool credential.

Versioned managed guidance asks new Agents to publish specifications, reference
documents and substantial explanations automatically. The server tool
instructions carry the same policy. Existing conversation tool catalogs must
be refreshed only at a fenced idle boundary, preserving the original conversation
and current owner settings; an active turn is never restarted for discovery.

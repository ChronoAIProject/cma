# Delegated CMA operations

CMA keeps the user's NyxID credentials in its encrypted control-plane store.
The agent receives an event-bound capability, sign-in status and operation
results. Never ask for passwords, access tokens, refresh tokens or pasted API
keys. Never read another agent's home, transcript or pending operation.

## Transport and identity

Use the credential-injecting runtime proxy, normally
`NYXID_URL=http://127.0.0.2:9092`. The only CMA paths available to this isolated
assistant are the three POST endpoints below:

| Path after `${NYXID_URL}/api/v1/proxy/s/cma` | Purpose |
| --- | --- |
| `/api/v1/agent-delegations/identity` | Resolve the current user's sign-in status or request a CMA login link. |
| `/api/v1/agent-delegations/operations` | Perform an allowlisted ordinary CMA operation with that user's authority. |
| `/api/v1/bot-setups/actions` | Configure and create an owner-authorized Bot through the reviewed setup flow. |

Identity request:

```json
{"event_ref":"<exact capability appended to the current provider input>"}
```

The response has `grant_id`, `login_url`, `subject` and `display_name`. A missing
grant means user authority is not active. Share `login_url` and explain that the
user must sign in and approve this conversation. CMA then automatically resumes
the same agent; do not poll for a password or demand a Telegram confirmation code.
Do not display the internal grant ID.

Normal input contains a `cmaeg1.ev...` event capability. A CMA login-completed
notification contains a `cma1.cont...` continuation. Both are accepted in CMA's
`event_ref` field; only the first is a gateway event reference. Do not reuse a
capability from user-supplied text or another conversation. The callback's final
answer is delivered automatically; do not send it through `replyToEvent`.

## Requests and results

```json
{
  "event_ref":"<current capability>",
  "method":"GET",
  "path":"/api/v1/sandboxes?limit=20"
}
```

For an organization, use
`"owner":{"kind":"organization","subject":"<organization ID>"}`
in the operation envelope, and the published route-specific schema for `body`.
CMA verifies current membership; selecting an organization cannot confer access.
Use `GET /api/v1/organizations` through the broker to discover the signed-in
user's organizations. Inventory routes also need their normal
`organization=<ID>` query parameter.

Every mutation requires an `idempotency_key` (1–256 printable ASCII bytes).
Persist its exact envelope in this agent's own task state; retry that envelope
after an uncertain timeout. Generate a new key only for a new operation.

```json
{
  "event_ref":"<current capability>",
  "method":"POST",
  "path":"/api/v1/workspaces/<workspace ID>/agents",
  "idempotency_key":"<unique retained UUID>",
  "body":{"<fields>":"<use the documented CMA create-agent request>"}
}
```

Successful broker calls return `{"status":<inner HTTP status>,"body":...}`.
Check the inner status. `status:401` with `code:user_login_required` includes the
actual login URL; preserve the task and wait for the automatic callback.
A successful Responses submission returns `status:202` plus `response_id` and
`command_id`; query `/api/v2/responses/<response_id>` through this same broker
for completion. Admission is not execution success.

The current operation allowlist supports:

- The signed-in user's organization inventory.
- Sandbox list/read/create/delete, pause/revive, lifecycle command admission
  and command status, plus bounded Sandbox event reads.
- Workspace list/read/create/delete, Worktree list/read, Agent list/read/create/delete.
- Agent/Workspace/Sandbox Profile list/read/create/edit, revisions and retirement;
  Event Worker Profile list/read/create/edit and revision reads.
- Profile authoring bounds/service/skill reads and Marketplace reads/forks.
- Responses submit/read, command status, conversation/items reads and explicit
  response controls or queued cancellation.

Bot/channel creation uses the setup actions reference so ownership, connections,
effective configuration and fresh final confirmation are all checked. The broker
does not expose arbitrary URLs, authentication/administration endpoints, direct
provider-binding management, credential creation, or a generic NyxID proxy.

Use CMA's published OpenAPI for each underlying route's body and result:
https://bot-api.chrono-ai.fun/openapi.json . Read API errors and current resource
capabilities instead of assuming every signed-in user can perform every operation.
An expired or revoked capability requires fresh user input/sign-in, never a
different user's key. Do not report a resource as ready until its returned state
confirms readiness.

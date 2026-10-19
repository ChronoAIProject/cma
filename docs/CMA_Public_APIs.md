# CMA public API inventory

The focused Agent Manager contract is served at `/agent-manager/openapi.json`;
see [Agent Manager API](API_Agent_Manager.md) for one Agent lifecycle and Responses v2 flow.
The full `/openapi.json` compatibility contract and existing API paths remain available.

This inventory describes the implemented CMA control-plane surface after legacy
Bridge retirement. `GET /openapi.json` is the authoritative machine-readable
contract; [REST](API_REST.md), [AG-UI](API_AGUI.md), and the Responses documents
provide request, replay and error semantics. CMAEG remains a separate service,
exposed under `/event-gateway` on the same origin with its own
[protocol](CMAEG_Protocol.md) and `/event-gateway/openapi.json` contract.

## Interface families

| Family | What clients can do |
| --- | --- |
| Profiles and marketplace | Create, edit, publish, inspect, fork and retire Sandbox, Workspace and Agent Profiles; discover public revisions. |
| Event Worker Profiles | Compose full worker configurations, retain immutable revisions and select them for Bots. |
| Hierarchy and lifecycle | Manage Sandboxes, Workspaces, Worktrees and Agents; observe durable commands and lifecycle events. |
| Conversations | Use AG-UI or Responses, observe/replay Agent events, act on supported approvals and controls. |
| Sharing and organizations | List accessible resources and manage supported resource grants under current authority. |
| Bots | Create and manage owner Bots through CMAEG, inspect progress, test, enable, disable, rotate credentials and delete. |
| Triggers and Launches | Publish visitor entry points, placements and embeds; admit and observe visitor-owned work and service connections. |
| Provider and gateway | Manage CMAEG provider bindings; admit provider turns and retrieve immutable full event context. |
| Operations | Manage platform settings, defaults and authorized inventory; inspect activity and conformance. |
| Auth and discovery | Browser sign-in/session/logout, OpenAPI, conformance and Agent-card discovery. |

Authentication is required except for explicitly anonymous discovery, public
marketplace and Trigger entry operations. Published does not mean anonymously
writable. Provider management requires dedicated service
capabilities; a personal bearer does not imply permission to call every route.

The legacy Gateway channel/connection/interaction storage endpoints are retired
from Agent Manager. Event Gateway owns that persistence directly; compatibility
SDK methods and fixtures remain for migration testing.

CMA PTY, MCP execution and A2A execution currently return typed unsupported
responses. Their discovery/refusal routes are listed for completeness. CMAEG’s
native MCP event tools are implemented independently.

## Exact method and path inventory

The generated OpenAPI document defines the full operation and path inventory.
Each row below names a public method and path; schemas, authentication schemes,
request parameters and error contracts are in `GET /openapi.json`. Disabled
features and typed unsupported operations are included because they remain part
of the public contract.

| API family | Operations |
| --- | ---: |
| administration | 14 |
| ag ui | 4 |
| agent events v2 | 3 |
| agent operations | 4 |
| agent profiles | 9 |
| agent usage | 2 |
| agents | 17 |
| bots | 9 |
| discovery | 2 |
| event gateway | 0 |
| event worker profiles | 6 |
| profile authoring | 4 |
| profile marketplace | 3 |
| profiles | 1 |
| provider bindings | 6 |
| resource access | 7 |
| response controls v2 | 5 |
| responses | 7 |
| responses v2 | 7 |
| sandbox profiles | 9 |
| sandboxes | 12 |
| service connections | 2 |
| trigger launches | 8 |
| trigger placements | 4 |
| triggers | 13 |
| workspace profiles | 9 |
| workspaces | 4 |
| worktrees | 2 |

### administration

| Method and path |
| --- |
| `GET /api/v1/admin/activity` |
| `GET /api/v1/admin/profiles` |
| `POST /api/v1/admin/profiles/{kind}/{id}/retire` |
| `GET /api/v1/admin/runtime-inventory` |
| `GET /api/v1/admin/sandboxes` |
| `GET /api/v1/admin/sandboxes/{id}` |
| `GET /api/v1/admin/sandboxes/{id}/events` |
| `POST /api/v1/admin/sandboxes/{id}/retire` |
| `GET /api/v1/admin/settings` |
| `PATCH /api/v1/admin/settings` |
| `PUT /api/v1/admin/settings` |
| `POST /api/v1/admin/settings/reset` |
| `POST /api/v1/admin/triggers/pause` |
| `POST /api/v1/admin/triggers/resume` |

### ag ui

| Method and path |
| --- |
| `GET /api/v1/agui/agents/{id}/sandbox` |
| `GET /api/v1/agui/capabilities` |
| `GET /api/v1/agui/events` |
| `POST /api/v1/agui/run` |

### agent events v2

| Method and path |
| --- |
| `GET /api/v2/agents/{agent_id}/events` |
| `GET /api/v2/agents/{agent_id}/events/snapshot` |
| `GET /api/v2/ui/transport` |

### agent operations

| Method and path |
| --- |
| `POST /api/v1/agents/{id}/approvals/{request_id}` |
| `POST /api/v1/agents/{id}/input` |
| `POST /api/v1/agents/{id}/interrupt` |
| `POST /api/v1/agents/{id}/steer` |

### agent profiles

| Method and path |
| --- |
| `GET /api/v1/agent-profiles` |
| `POST /api/v1/agent-profiles` |
| `GET /api/v1/agent-profiles/{id}` |
| `PUT /api/v1/agent-profiles/{id}` |
| `POST /api/v1/agent-profiles/{id}/retire` |
| `GET /api/v1/agent-profiles/{id}/revisions` |
| `POST /api/v1/agent-profiles/{id}/revisions` |
| `GET /api/v1/agent-profiles/{id}/revisions/{revision}` |
| `PUT /api/v1/agent-profiles/{id}/visibility` |

### agent usage

| Method and path |
| --- |
| `GET /api/v1/agents/{id}/resources` |
| `GET /api/v1/agents/{id}/usage` |

### agents

| Method and path |
| --- |
| `GET /api/v1/agents` |
| `GET /api/v1/agents/provenance` |
| `DELETE /api/v1/agents/{id}` |
| `GET /api/v1/agents/{id}` |
| `GET /api/v1/agents/{id}/artifacts` |
| `POST /api/v1/agents/{id}/artifacts` |
| `GET /api/v1/agents/{id}/artifacts/{artifact_id}` |
| `GET /api/v1/agents/{id}/artifacts/{artifact_id}/download` |
| `GET /api/v1/agents/{id}/channel-usage` |
| `GET /api/v1/agents/{id}/channel-usage/history` |
| `GET /api/v1/agents/{id}/events` |
| `GET /api/v1/agents/{id}/provenance` |
| `GET /api/v1/agents/{id}/source` |
| `POST /api/v1/agents/{id}/stop` |
| `GET /api/v1/agents/{id}/terminal` |
| `GET /api/v1/agents/{id}/transcript` |
| `POST /api/v1/workspaces/{id}/agents` |

### bots

| Method and path |
| --- |
| `GET /api/v1/bots` |
| `POST /api/v1/bots` |
| `GET /api/v1/bots/connections` |
| `GET /api/v1/bots/nyxid-bots` |
| `DELETE /api/v1/bots/{bot_id}` |
| `GET /api/v1/bots/{bot_id}` |
| `POST /api/v1/bots/{bot_id}/disable` |
| `POST /api/v1/bots/{bot_id}/enable` |
| `POST /api/v1/bots/{bot_id}/rotate-keys` |
| `POST /api/v1/bots/{bot_id}/test` |

### discovery

| Method and path |
| --- |
| `GET /api/v1/conformance` |
| `GET /auth/signup-options` |

### event gateway

| Method and path |
| --- |

### event worker profiles

| Method and path |
| --- |
| `GET /api/v1/event-worker-profiles` |
| `POST /api/v1/event-worker-profiles` |
| `DELETE /api/v1/event-worker-profiles/{id}` |
| `GET /api/v1/event-worker-profiles/{id}` |
| `PUT /api/v1/event-worker-profiles/{id}` |
| `GET /api/v1/event-worker-profiles/{id}/revisions/{revision}` |

### profile authoring

| Method and path |
| --- |
| `GET /api/v1/profile-authoring/bounds` |
| `POST /api/v1/profile-authoring/repositories/resolve` |
| `GET /api/v1/profile-authoring/services` |
| `GET /api/v1/profile-authoring/skills` |

### profile marketplace

| Method and path |
| --- |
| `GET /api/v1/marketplace/{kind}` |
| `GET /api/v1/marketplace/{kind}/{id}` |
| `POST /api/v1/marketplace/{kind}/{id}/fork` |

### profiles

| Method and path |
| --- |
| `GET /api/v1/defaults` |

### provider bindings

| Method and path |
| --- |
| `DELETE /api/v1/cmaeg/bindings/{binding_id}` |
| `GET /api/v1/cmaeg/bindings/{binding_id}` |
| `PUT /api/v1/cmaeg/bindings/{binding_id}` |
| `DELETE /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` |
| `PUT /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}` |
| `GET /api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}/events/{event_id}` |

### resource access

| Method and path |
| --- |
| `GET /api/v1/organizations` |
| `GET /api/v1/resource-access/{kind}/{id}` |
| `GET /api/v1/resource-shares/{kind}/{id}` |
| `POST /api/v1/resource-shares/{kind}/{id}` |
| `DELETE /api/v1/resource-shares/{kind}/{id}/{share_id}` |
| `GET /api/v1/resource-shares/{kind}/{id}/{share_id}` |
| `PATCH /api/v1/resource-shares/{kind}/{id}/{share_id}` |

### response controls v2

| Method and path |
| --- |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/controls` |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/controls` |
| `GET /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` |
| `POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-cancellation` |
| `GET /api/v2/commands/{command_id}` |

### responses

| Method and path |
| --- |
| `POST /api/v1/conversations` |
| `GET /api/v1/conversations/{id}` |
| `GET /api/v1/conversations/{id}/items` |
| `POST /api/v1/responses` |
| `DELETE /api/v1/responses/{response_id}` |
| `GET /api/v1/responses/{response_id}` |
| `POST /api/v1/responses/{response_id}/cancel` |

### responses v2

| Method and path |
| --- |
| `GET /api/v2` |
| `POST /api/v2/agents/{agent_id}/responses` |
| `GET /api/v2/conversations/{conversation_id}` |
| `GET /api/v2/conversations/{conversation_id}/items` |
| `POST /api/v2/responses` |
| `GET /api/v2/responses/{response_id}` |
| `GET /api/v2/responses/{response_id}/events` |

### sandbox profiles

| Method and path |
| --- |
| `GET /api/v1/sandbox-profiles` |
| `POST /api/v1/sandbox-profiles` |
| `GET /api/v1/sandbox-profiles/{id}` |
| `PUT /api/v1/sandbox-profiles/{id}` |
| `POST /api/v1/sandbox-profiles/{id}/retire` |
| `GET /api/v1/sandbox-profiles/{id}/revisions` |
| `POST /api/v1/sandbox-profiles/{id}/revisions` |
| `GET /api/v1/sandbox-profiles/{id}/revisions/{revision}` |
| `PUT /api/v1/sandbox-profiles/{id}/visibility` |

### sandboxes

| Method and path |
| --- |
| `GET /api/v1/sandbox-commands/{id}` |
| `GET /api/v1/sandboxes` |
| `POST /api/v1/sandboxes` |
| `DELETE /api/v1/sandboxes/{id}` |
| `GET /api/v1/sandboxes/{id}` |
| `POST /api/v1/sandboxes/{id}/commands` |
| `GET /api/v1/sandboxes/{id}/events` |
| `POST /api/v1/sandboxes/{id}/pause` |
| `POST /api/v1/sandboxes/{id}/revive` |
| `POST /api/v1/sandboxes/{id}/runtime-update` |
| `POST /api/v1/sandboxes/{id}/service-access` |

### service connections

| Method and path |
| --- |
| `POST /api/v1/service-connections` |
| `GET /api/v1/service-connections/{handoff_id}` |

### trigger launches

| Method and path |
| --- |
| `GET /api/v1/agui/launches/{id}/events` |
| `GET /api/v1/launches` |
| `GET /api/v1/launches/{id}` |
| `POST /api/v1/launches/{id}/cancel` |
| `GET /api/v1/launches/{id}/events` |
| `GET /api/v1/launches/{id}/requirements` |
| `POST /api/v1/launches/{id}/start` |
| `POST /api/v1/triggers/public/{public_id}/launches` |

### trigger placements

| Method and path |
| --- |
| `GET /api/v1/triggers/{id}/embed` |
| `GET /api/v1/triggers/{id}/placements` |
| `POST /api/v1/triggers/{id}/placements` |
| `POST /api/v1/triggers/{id}/placements/{placement_id}/retire` |

### triggers

| Method and path |
| --- |
| `GET /api/v1/triggers` |
| `POST /api/v1/triggers` |
| `GET /api/v1/triggers/public/{public_id}` |
| `GET /api/v1/triggers/public/{public_id}/badge.svg` |
| `POST /api/v1/triggers/public/{public_id}/entry-intent` |
| `GET /api/v1/triggers/public/{public_id}/open/{placement_id}` |
| `DELETE /api/v1/triggers/{id}` |
| `GET /api/v1/triggers/{id}` |
| `PUT /api/v1/triggers/{id}` |
| `POST /api/v1/triggers/{id}/publish` |
| `POST /api/v1/triggers/{id}/retire` |
| `GET /api/v1/triggers/{id}/revisions` |
| `GET /api/v1/triggers/{id}/stats` |

### workspace profiles

| Method and path |
| --- |
| `GET /api/v1/workspace-profiles` |
| `POST /api/v1/workspace-profiles` |
| `GET /api/v1/workspace-profiles/{id}` |
| `PUT /api/v1/workspace-profiles/{id}` |
| `POST /api/v1/workspace-profiles/{id}/retire` |
| `GET /api/v1/workspace-profiles/{id}/revisions` |
| `POST /api/v1/workspace-profiles/{id}/revisions` |
| `GET /api/v1/workspace-profiles/{id}/revisions/{revision}` |
| `PUT /api/v1/workspace-profiles/{id}/visibility` |

### workspaces

| Method and path |
| --- |
| `GET /api/v1/sandboxes/{id}/workspaces` |
| `POST /api/v1/sandboxes/{id}/workspaces` |
| `DELETE /api/v1/workspaces/{id}` |
| `GET /api/v1/workspaces/{id}` |

### worktrees

| Method and path |
| --- |
| `GET /api/v1/workspaces/{id}/worktrees` |
| `GET /api/v1/worktrees/{id}` |

Event Worker Profile creation uses an `Idempotency-Key`; updates and retirement
use `expected_revision`. Retiring a worker preserves existing Bot pins and
historical revisions. The complete configuration is returned in `profile`.

## Supplemental browser, discovery and operational routes

These router surfaces supplement the 180 OpenAPI operations above. Browser
identity and connection flows retain their cookie, redirect and same-origin
requirements; they are not generic resource CRUD. `GET /auth/signup-options`
and the public Trigger embed routes are already included in the OpenAPI table.

| Method and path | Implemented behavior |
| --- | --- |
| `GET /auth/login` | Start browser OAuth sign-in or registration. |
| `GET /auth/callback` | Complete the browser identity exchange. |
| `GET /auth/session` | Read the signed-in subject and capabilities. |
| `GET /auth/logout` | End browser sign-in and redirect through logout. |
| `POST /auth/connections/check` | Check the owner's selected service connection. |
| `POST /auth/connections/start` | Start the browser service connection flow. |
| `POST /auth/connections/cancel` | Cancel the retained connection intent. |
| `GET /openapi.json` | Read the machine-readable CMA contract. |
| `GET /.well-known/agent-card.json` | Discover the CMA provider and its capabilities. |
| `POST /api/v1/mcp` | Return the explicit unsupported-interface response. |
| `ANY /api/v1/a2a` | Return the explicit unsupported-interface response. |
| `GET /livez` | Probe API process liveness. |
| `GET /ready` | Probe API readiness, including database availability. |

CMAEG's supported native `POST /mcp` belongs to the gateway origin and is a
separate interface from CMA's unsupported `/api/v1/mcp` route.

## Agent model catalog

`GET /api/v1/agents/{id}/models` (`list_agent_models`) returns
`{"models":[{"id":"gpt-6","label":"gpt-6","available":true}]}` using the owned Agent’s
current Sandbox key to query the Chrono LLM `/models` API. The portal uses the
Agent Manager prefix `/api/agent-manager/v1/agents/{id}/models`. The response is
`Cache-Control: no-store`; the list contains provider results without adding
hardcoded choices. Labels prefer the provider's `display_name`, then `name`,
then the model ID. The existing settings PATCH applies the selected model.

Chrono's current `/models` response includes image-only models and does not
provide modality or supported-endpoint metadata (`type` is `model` for every
entry). The Agent catalog therefore admits only provider-returned IDs matching
`^gpt-[0-9][A-Za-z0-9._-]*$` or exactly `codex-auto-review`, the same ID contract
used by managed ACP model discovery. This excludes `gpt-image-*` and unknown
model families. Each row's `available` flag additionally requires an active
Agent and parent, no checkpoint capture, and a fresh status from the exact bound
runtime reporting `agent_model_catalog: true`, ready installation and credential,
and matching generation, home identity and boot ID. Missing capability, status
failure, or a retained older runtime leaves rows visible with `available: false`.
The portal disables those model choices with an explanation, preserves the saved
model and still allows reasoning effort and permission changes. A missing flag
from an older API also means unavailable. Catalog reads never upgrade, revive,
or replace a running Agent or its conversation.

Requests use an explicit `chrono-cma/<version>` User-Agent and
`Accept: application/json`. Production verification on 2026-10-14 confirmed
that the existing scoped Sandbox key can read `/models`: a Python-urllib
User-Agent was denied by the upstream perimeter, while no User-Agent and a
normal CMA client User-Agent returned the same 20 model IDs. The route does not
need `/v1/models` (which returned 404), an owner-token fallback, or wider scope.

The caller must own the Agent. Foreign or missing Agents return 404 before any
upstream request. Missing, expired, or mismatched Sandbox credential custody,
invalid provider responses, and upstream failures return 503 without exposing
credentials or the upstream response body. Public Chrono LLM scope is preferred;
retained Sandboxes with the legacy Chrono LLM scope use their original service.


### Queued input revisions

`GET /api/v2/agents/{agent_id}/queued-inputs` requires current Agent operation
authority and returns `cma.agent.queued_inputs`, the Agent ID and at most four entries
ordered by original admission order. Optional `limit` is 1 through 4 (default 4);
optional `after` is an exclusive positive admission-order cursor. `next_after` is
the final returned order when more entries exist, otherwise null. Each entry includes
the safe command status, `command_revision`, complete `text` and current `editable`
observation. Four maximum 256 KiB texts remain below the SDK's unchanged 8 MiB HTTP
limit even with worst-case JSON escaping. The SDKs follow pages with strict cursor,
identity and 64-entry traversal bounds. Each page observes current state; later
refreshes reconcile concurrent edits. No entries are silently truncated.

`POST /api/v2/agents/{agent_id}/responses/{response_id}/queued-input` requires a
retained `Idempotency-Key` and exactly `source_command_id`,
`expected_command_revision` and `text`. Text obeys the existing nonempty 256 KiB
input limit. The transaction retains the original admission envelope, command ID,
response ID, metadata and queue position; it atomically invalidates any matching
claim, records the replacement input and increments the source revision. A bound
runtime dispatch is never editable, including ambiguous delivery. The independent
terminal edit receipt allows exact recovery after a lost reply. Another key cannot
reuse the old revision, and the same key cannot change target or text.

The response is `cma.response.queued_input` with `edit`, current `response`, original
`target`, applied `command_revision` and immutable replacement `text`.
`queued_input_not_admitted` is a committed stale-intent refusal; clients may refresh
and explicitly create a new edit. Existing queued cancellation removes an item.
Queue changes before dispatch are ledger mutations, so visible clients refresh
periodically as well as on local operations and focus.

Pending edited command documents include a required revision marker that older
strict readers reject. This prevents a rolled-back worker from silently executing
original input. Drain or cancel edited pending inputs before rolling back to a
release without queued input support. Forward recovery reads the retained revisions;
active or completed dispatch records remain compatible after the marker is removed.

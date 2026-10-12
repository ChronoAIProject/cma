# CMA public API inventory

This inventory describes the implemented CMA control-plane surface after legacy
Bridge retirement. `GET /openapi.json` is the authoritative machine-readable
contract; [REST](API_REST.md), [AG-UI](API_AGUI.md), and the Responses documents
provide request, replay and error semantics. CMAEG has a separate origin and
[protocol](CMAEG_Protocol.md); its endpoints are not CMA endpoints.

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

The generated OpenAPI document contains **180 operations across 144 paths**.
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
| sandboxes | 11 |
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

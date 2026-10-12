# CMA service isolation

Revised target, 2026-10-12. See [architecture](CMA_Service_Split.md).

## Independent services

| Service | NyxID identity / permission prefix | Data |
|---|---|---|
| Event Gateway | `cma-event-gateway` browser/management identity; legacy `cmaeg` tool identity retained; `cma-eg:` permissions | Channels, encrypted records and gateway delivery state |
| Agent Manager | Existing CMA identity and `cma:` permissions retained for Agent API compatibility | Agents, runtime bindings, workspaces, profiles, turns, messages, approvals, checkpoints and application auth |
| Image Manager | `cma-image-manager`; `cma-im:` | Image catalog, shares, versions, builds and retention references |

Each service owns its OAuth client/secret, exact target resource, role catalog,
DB URI, encryption key, schema validators, indexes and migrations. Admin in one
service is not admin in another. Legacy unscoped permissions do not implicitly
grant the new service's permissions. User/role assignment authority remains NyxID.
Service-to-service requests identify both caller and authorized resource scope;
an arbitrary owner field is not identity delegation.

There is no Sandbox Manager, its permissions or a shared runtime database.
Agent Manager directly uses Kubernetes Sandbox resources and owns its runtime
mapping. Image Manager never reads Agent collections; Agent Manager never reads
Image collections. Gateway's sealed records move out of the Agent database into
Gateway's own schema. Cross-service transactions, lookup and change streams are
prohibited. Copies obtained through APIs are projections, not another service's
writable authority.

## Database isolation

Shared MongoDB infrastructure is allowed. Prefer separate logical databases for
clear ownership; exact collection-scoped custom roles in one database are also
valid. No runtime account may read/write another service's collections or hold global
admin roles. Image and Gateway migrations use separate identities. Agent Manager
retains its existing startup index/schema initialization, with DDL confined to its
explicit collection catalog; extracting those migrations is a separate change. Each service verifies effective grants at startup.
Migrators have only their service's explicitly named schema permissions.

Existing Agent collections can retain their names in an Agent-only database;
renaming every table is not a substitute for permission isolation. New image
collections use `im_`; gateway records use `eg_`. Migration must preserve IDs,
versions, encrypted payloads, histories, unique indexes and retry identities.
Legacy access is removed only after verified authority transfer.

## Frontend and infrastructure

Existing web supplies `/event-gateway`, `/agent-manager`, `/image-manager`
entrances. Each has a distinct backend client and permission/query state. Shared
login/design components do not merge authorization. An unavailable Image Manager
must not hide Agent history or terminate an already running Agent.

Agent Manager's Kubernetes service account accesses only its configured runtime
namespace, with server-side CMA selectors and object ownership/UID verification.
Kubernetes RBAC cannot restrict list by labels; do not describe label filtering
as RBAC enforcement. Use a dedicated CMA namespace where infra permits it, plus
admission policies for deployment labels and privileged pod settings.

Image Manager has its own Google build/registry identity, restricted to the
configured Artifact Registry repository. The cluster's pull identity is separate.
Registry credentials are never frontend configuration or sandbox environment.
Sandbox controllers and CRDs remain infra-owned; manifests consume their API.


## Implemented identities and collections

Browser callbacks are `/api/event-gateway/auth/callback` and
`/api/image-manager/auth/callback` on the configured web origin. Each uses an
independent confidential NyxID client, resource, 32-byte encryption key and
`__Host-` HttpOnly cookie. OAuth state and PKCE verifier are consumed once.
Encrypted tokens stay in `eg_auth_sessions` / `im_auth_sessions`; sessions have
TTL indexes, an enforced maximum one-hour lifetime, per-request introspection,
and exact-origin CSRF checks on cookie-authenticated writes. Logout is local to
the selected service. No service reads another service's login collections.

The Image Manager schema is `im_images`, `im_builds`, `im_schema_migrations`,
`im_auth_sessions`. Successful immutable builds are the version catalog; image
retirement retains their registry blobs for already admitted Agents. The Gateway
schema is `eg_channel_records`, `eg_schema_migrations`, `eg_auth_sessions`.
AM's full exact collection catalog is owned by `store/isolation.rs`; the legacy
`gateway_channel_records` collection is excluded from its runtime grants.
The startup migration of `binding_conversations` alone requires `dropIndex`;
that action is refused on every other collection. The catalog includes runtime
retained availability, provider tool opens, actor links and lifecycle operations
so the same restricted credential can initialize and operate the actual stores.

`cma --print-runtime-role`, `cmaeg --print-runtime-role` and
`cma-image-manager --print-runtime-role` emit collection-scoped roles. The latter
two also support `--print-migration-role` and `--migrate`. None creates users or
assigns NyxID roles implicitly. Provision those identities before rollout.


兼容的 NyxID 个人 access token 可以没有 RFC 8707 resource 限制；这类 token
仍必须具有目标服务自己的 `cma-eg:*` / `cma-im:*` permission。显式限制到其他
resource 的 token 一律拒绝。AM 已受理的 Bot 工作可继续使用其保管的用户授权，
但 `cma:admin` 不隐含 EG 或 IM 的权限；部署前须分配对应服务角色。

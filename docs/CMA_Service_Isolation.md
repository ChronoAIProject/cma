# CMA service isolation

Revised deployment, 2026-10-13. See [architecture](CMA_Service_Split.md).

## Independent services

| Service | NyxID identity / permission prefix | Data |
|---|---|---|
| Event Gateway | `cma-event-gateway` browser/management identity; legacy `cmaeg` tool identity retained; `cma-eg:` permissions | Channels, encrypted records and gateway delivery state |
| Agent Manager | Existing CMA identity and `cma:` permissions retained for Agent API compatibility | Agents, runtime bindings, workspaces, profiles, turns, messages, approvals, checkpoints and application auth |
| Image Manager | `cma-image-manager`; `cma-im:` | Image catalog, shares, versions, builds and retention references |

Each service owns its OAuth client/secret, exact target resource, role catalog,
encryption key, schema validators, indexes and migrations. The current deployment
uses the same `chrono-bot` database URI with separate service schemas. Admin in one
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

The deployment requested on 2026-10-13 uses the existing `chrono-bot` database
and database-scoped credential for all three services. Agent Manager retains its
exact collection catalog, Image Manager owns `im_*`, and Event Gateway owns
`eg_*`. Their schema versions, validators, indexes, sessions and migrations
remain independent. No service store opens another service's collections, and
no cross-service database joins or transactions are introduced.

Each binary defaults to `isolated`, which verifies exact collection-scoped
effective grants. The checked-in deployment explicitly selects `shared-schema`
with `CHRONO_MONGODB_AUTHORITY_MODE`, `CMA_IM_MONGODB_AUTHORITY_MODE`, and
`CMAEG_MONGODB_AUTHORITY_MODE`. All database names are `chrono-bot`. Shared mode
checks authenticated database authority and all operations required by that
service, permits the existing database-wide grants, and still refuses global or
other-database grants. It does not provide account-level isolation between the
services; schema and application boundaries enforce their ownership in this
mode. Switching to separate restricted credentials and `isolated` requires no
collection renaming or data movement.

Image and Gateway migrations are explicit commands targeting only their own
collections; Agent Manager retains startup index/schema initialization confined
to its named catalog. Validators, unique indexes and migration markers remain
mandatory in both authority modes. Copying the database URI into each service's
Secret does not share browser keys or OAuth identities.

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

Agent Manager and Event Gateway share Agent Manager's existing NyxID client,
`/auth/login` and `/auth/logout` flow, and HttpOnly CMA browser session. The
`/api/event-gateway` bridge resolves the browser session in Agent Manager and
forwards its server-held bearer; Event Gateway independently introspects that
bearer and applies its own permissions and record ownership checks. No service
reads another service's login collections. Historical `eg_auth_sessions` expire
normally and are not accepted by the shared portal. See `Shared_Login.md` for
the bridge's exact-origin checks, token custody, and route contract. The canonical
`/event-gateway/v1`, `/event-gateway/mcp` and `/event-gateway/openapi.json` namespace
routes directly to Event Gateway with its own API credentials. It neither bypasses
the cookie bridge nor grants another service's permissions.

Image Manager retains its independent `/api/image-manager/auth/callback`, client,
resource, encryption key and `__Host-` HttpOnly cookie for its later rollout.
OAuth state and PKCE verifier are consumed once. Encrypted tokens remain in
`im_auth_sessions`, with TTL indexes, a maximum one-hour lifetime, per-request
introspection and exact-origin CSRF checks. Image Manager logout is local to it.

The Image Manager schema is `im_images`, `im_builds`, `im_schema_migrations`,
`im_auth_sessions`. Successful immutable builds are the version catalog; image
retirement retains their registry blobs for already admitted Agents. The Gateway
schema is `eg_channel_records`, `eg_connection_records`, `eg_interaction_records`,
`eg_schema_migrations` and `eg_auth_sessions` (schema marker 2). Each record kind
uses its own collection; interactions preserve immutable expiration and a TTL
index, and expired records are refused before asynchronous TTL cleanup.
AM's full exact collection catalog is owned by `store/isolation.rs`; the legacy
`gateway_channel_records` collection is excluded from its runtime grants.
Agent Manager no longer initializes or serves legacy channel, connection or
interaction record storage. Event Gateway uses its own `eg_channel_records` store;
the old adapters and protocol fixtures are retained only for migration regression
coverage. Provider bindings remain Agent Manager resources.
Agent Manager verifies its configured credential policy before any startup DDL. The
restricted bootstrap contract opens the full production store assembly twice,
including Bot setups and Event Worker definitions, while reads of all three
legacy Gateway collections remain forbidden.
The startup migration of `binding_conversations` alone requires `dropIndex`;
that action is refused on every other collection. The catalog includes runtime
retained availability, provider tool opens, actor links and lifecycle operations
so the same restricted credential can initialize and operate the actual stores.

`cma --print-runtime-role`, `cmaeg --print-runtime-role` and
`cma-image-manager --print-runtime-role` emit collection-scoped roles. The latter
two also support `--print-migration-role` and `--migrate`. None creates users or
assigns NyxID roles implicitly. Provision those identities before rollout.

### Sandbox funding grants

NyxID funding adds one Agent Manager collection, `sandbox_billing_state`, with
the standard unique `_id_` index and the non-unique `sandbox_billing_due` index
on `{retry_at: 1, _id: 1}`. Existing `sandbox_usage` and
`sandbox_usage_events` remain in the Agent Manager catalog. Startup initializes
the funding collection and its index even when the funding adapter is disabled.

The generated `cma_am_runtime` role therefore grants `find`, `insert`, `update`,
`remove`, `createCollection`, `createIndex`, `listIndexes` and `collMod` on
`sandbox_billing_state`, following Agent Manager's startup DDL policy. It grants
no `dropIndex` on that collection and no database-wide data access. Agent Manager
has no separate migrator role; Gateway and Image Manager runtime/migrator roles
must not receive funding collection privileges.

Before rolling out this binary with isolated credentials, regenerate the role
with `CHRONO_MONGODB_DATABASE=chrono-bot cma --print-runtime-role`. An authorized
database operator must reconcile the existing role's privileges from that
reviewed output before starting the new binary; printing the role does not
update it. For an existing `cma_am_runtime`, use MongoDB's `updateRole` command
with the generated `privileges` and `roles` fields, rather than rerunning
`createRole`. For a fresh role, use the generated `createRole` command and assign
it to the Agent Manager identity. Preserve the database and existing records.
The old role fails the new binary's authority check before store initialization.
Keep `CHRONO_NYXID_SANDBOX_BILLING_ENABLED=false` and
`CHRONO_USAGE_BILLING_ENABLED=false`; granting storage authority does not
activate either billing adapter.


兼容的 NyxID 个人 access token 可以没有 RFC 8707 resource 限制；这类 token
仍必须具有目标服务自己的 `cma-eg:*` / `cma-im:*` permission。显式限制到其他
resource 的 token 一律拒绝。AM 已受理的 Bot 工作可继续使用其保管的用户授权，
但 `cma:admin` 不隐含 EG 或 IM 的权限；部署前须分配对应服务角色。

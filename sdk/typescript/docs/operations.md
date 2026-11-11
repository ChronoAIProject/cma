# Operations

<!-- nav: 9 Operations -->

The `admin` namespace requires explicit operator capabilities. Ordinary resource
helpers remain owner-scoped; possessing a client does not grant operator access.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
const deployment = await cma.readConformance();
const openapi = await cma.readOpenApi();
const inventory = await cma.admin.readRuntimeInventory();
console.log(deployment.interfaces, Object.keys(openapi.paths), inventory);
```

Conformance and OpenAPI reads suppress credentials. Runtime inventory is an
authenticated operator read. Do not treat a configured image reference or an
operator's declared commit as verified live runtime provenance.

`listSandboxes`, `readSandbox`, `sandboxEvents`, `listProfiles` and `listActivity`
provide cross-owner inspection according to the caller's capabilities.
Sandbox and Profile inventory rows include `owner_name`; Sandbox detail includes
the creator's `owner_name` beside `sandbox`. Activity includes `actor_name` and,
for user targets, `target_name`. These retained public names are nullable and
optional on older servers. Display an unavailable name explicitly; never use the
`owner`, `actor`, or target subject identifier as a person's name. The ordinary
Sandbox read also includes immutable `owner` and its current `owner_name`.
`retireSandbox` and `retireProfile` are explicit operator commands; they do not
change the semantics of owner-scoped deletion helpers.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient();
export async function restorePlatformDefaults() {
  const observed = await cma.admin.readSettings();
  return cma.admin.resetSettings({ expected_revision: observed.revision });
}
```

Settings replacement, patch and reset use `expected_revision`. Patches contain
at least one complete section; omitted sections remain unchanged, while null
sections and empty patches are refused. Reread after a conflict and review the
new values before issuing a new command. `pauseTriggers` and `resumeTriggers`
set `triggers.orchestration_paused`, freezing or resuming admitted Launch work
without failing it. `triggers.enabled` separately controls new public admission.
These commands do not invent an idempotency key or
automatically retry an uncertain result.

Settings reads and successful settings mutations preserve the optional
`model_budget_writes_available`, `model_request_budget_writes_available` and
`platform_spend_writes_available` booleans. These describe deployment
compatibility, separately from policy enablement and operator permissions.
False or missing means the corresponding save capability is unavailable; saving
nonempty request-model bounds requires the first two flags to be true, and
saving platform `cloud` spend fields requires the first and third. Keep this
read-only metadata outside writable settings.
The server remains authoritative and can refuse a save if availability changes.

Sandbox persistence is owned by Agent Sandbox and its attached PV. CMA exposes
no public setting for snapshot scheduling or checkpoint archive retention.

`unsupported.mcp()`, `.a2a()` and `.agentCard()` preserve typed interface refusals
as `ApiError`; they are probes, not implementations of those protocols. Agent
terminal requests are likewise refused by `agents.terminal(id)`.

## Native warm pools

Native pools reserve the deployment's pinned system runtime image and a CPU/memory allocation. The server owns the home seed and runtime admission policy. `configure_platform` is required for inventory and writes.

```ts
import { createClient } from '@chronoai/cma-sdk';

const cma = createClient({ baseUrl: 'https://cma.example' });
const inventory = await cma.admin.listWarmPools();
const target = await cma.admin.configureWarmPool(
  {
    cpu_millis: inventory.default_cpu_millis,
    memory_mib: inventory.default_memory_mib,
    target_replicas: 2,
  },
  { idempotencyKey: 'operator-pool-create-2026-10-13' },
);

// Accepted targets reconcile asynchronously. Keep the same key and request
// when retrying an uncertain creation response.
const refreshed = await cma.admin.listWarmPools();
const current = refreshed.items.find((pool) => pool.id === target.id);
if (current) {
  await cma.admin.resizeWarmPool(current.id, {
    expected_revision: current.revision,
    target_replicas: 0,
  });
}
```

`available: false` means provider inventory could not be observed. Recorded targets remain visible, and `ready_replicas: null` means readiness is unknown. A 202 response acknowledges durable intent; check `status` and `observed` for reconciliation progress. Refresh after a resize conflict before submitting a new revision. Setting the target to zero stops replenishment; claimed Agent runtimes keep their existing homes and lifecycle.

Pool inventory exposes each target's immutable `config_sha256` and the current
`default_config_sha256` for the default image, CPU, memory, home, and Pod policy.
Older servers omit these fields, which the SDK preserves as `undefined`.
Select default capacity only when both digests are present and equal;
matching only image and resources can select a retained older policy.
Use a new idempotency key to create a target after policy changes. Replaying an
old key and resizing an old target preserve that target's original policy.

## Task experience policy

`await client.taskExperience.read()` dispatches authenticated
`GET /api/v2/ui/task-experience` on every call. Its version1 result contains only
the caller's `account_id` and boolean `capabilities`: `task_home_admission`,
`suggested_followup_admission`, `desktop_enable`, `browser_takeover`, and
`replay_presentation`. Responses are private/no-store; malformed/unknown schemas
raise `ProtocolError`. Compare the caller id to the current viewer and refresh
after identity/focus changes. Unavailable/older services keep ordinary manual
conversation usable and do not authorize new task features.

These are policy permissions, separate from environment support and fresh resource
authority. Closing new admission preserves exact accepted operations and request
keys. Browser status/receipt recovery and explicit return remain available while
new acquire/input/renew is disabled; do not hide a held browser or assume release.
Replay eligibility affects presentation, not retained authorized history reads.

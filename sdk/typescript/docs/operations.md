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

`unsupported.mcp()`, `.a2a()` and `.agentCard()` preserve typed interface refusals
as `ApiError`; they are probes, not implementations of those protocols. Agent
terminal requests are likewise refused by `agents.terminal(id)`.
